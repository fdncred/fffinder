//! Terminal input: read on a thread of its own and turned into the input
//! events GPUI gets from a native window.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode,
    MouseButton as TermButton, MouseEvent, MouseEventKind,
};
use gpui_kit::{
    Capslock, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta,
    ScrollWheelEvent, TouchPhase, point, px,
};
use std::time::{Duration, Instant};

/// A second click within this time and distance counts toward a double click.
const MULTI_CLICK_TIME: Duration = Duration::from_millis(500);
const MULTI_CLICK_DISTANCE: f32 = 4.;

/// Read terminal events until the terminal goes away. The channel closes
/// when it does.
pub fn spawn_reader() -> async_channel::Receiver<Event> {
    let (tx, rx) = async_channel::unbounded();
    let spawned = std::thread::Builder::new()
        .name("fffinder-term-input".into())
        .spawn(move || {
            loop {
                match crossterm::event::read() {
                    Ok(event) => {
                        if tx.send_blocking(event).is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        log::error!("reading the terminal: {err}");
                        return;
                    }
                }
            }
        });
    if let Err(err) = spawned {
        log::error!("could not start the terminal reader: {err}");
    }
    rx
}

/// How terminal mouse positions map onto the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// One cell in device pixels.
    pub cell: (f32, f32),
    /// Positions are in pixels (mode 1016), not cells.
    pub pixels: bool,
}

impl Mapping {
    fn position(&self, column: u16, row: u16) -> Point<Pixels> {
        if self.pixels {
            // crossterm takes one off each coordinate, since cells count from
            // one, but pixels count from zero, so a 0 has wrapped to u16::MAX.
            let x = f32::from(column.wrapping_add(1));
            let y = f32::from(row.wrapping_add(1));
            point(px(x / self.scale), px(y / self.scale))
        } else {
            // The middle of the cell.
            point(
                px((f32::from(column) + 0.5) * self.cell.0 / self.scale),
                px((f32::from(row) + 0.5) * self.cell.1 / self.scale),
            )
        }
    }

    /// How far one wheel step scrolls: a cell's height, which is about how
    /// far a finger moves on a trackpad for the terminal to send one.
    fn wheel_step(&self) -> Pixels {
        px(self.cell.1 / self.scale)
    }
}

struct Click {
    button: MouseButton,
    at: Instant,
    position: Point<Pixels>,
    count: usize,
}

/// Turns terminal events into GPUI's, keeping what they leave out: which
/// modifiers are down, and how many clicks make up the current one.
#[derive(Default)]
pub struct Translator {
    modifiers: Modifiers,
    last_click: Option<Click>,
    position: Point<Pixels>,
}

impl Translator {
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    pub fn position(&self) -> Point<Pixels> {
        self.position
    }

    /// A key press, repeat, or release; modifier keys on their own become a
    /// change of modifiers.
    pub fn key(&mut self, event: KeyEvent) -> Option<PlatformInput> {
        let down = event.kind != KeyEventKind::Release;
        if let KeyCode::Modifier(key) = event.code {
            match key {
                ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift => {
                    self.modifiers.shift = down
                }
                ModifierKeyCode::LeftControl | ModifierKeyCode::RightControl => {
                    self.modifiers.control = down
                }
                ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt => self.modifiers.alt = down,
                ModifierKeyCode::LeftSuper | ModifierKeyCode::RightSuper => {
                    self.modifiers.platform = down
                }
                ModifierKeyCode::LeftHyper
                | ModifierKeyCode::RightHyper
                | ModifierKeyCode::LeftMeta
                | ModifierKeyCode::RightMeta
                | ModifierKeyCode::IsoLevel3Shift
                | ModifierKeyCode::IsoLevel5Shift => return None,
            }
            return Some(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                modifiers: self.modifiers,
                capslock: Capslock::default(),
            }));
        }
        let keystroke = keystroke(event.code, event.modifiers)?;
        // Shift isn't synced: crossterm drops it once it has applied it to
        // the key, as in `>` for shift-period.
        self.modifiers.control = keystroke.modifiers.control;
        self.modifiers.alt = keystroke.modifiers.alt;
        self.modifiers.platform = keystroke.modifiers.platform;
        Some(match event.kind {
            KeyEventKind::Press | KeyEventKind::Repeat => PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: event.kind == KeyEventKind::Repeat,
                prefer_character_input: false,
            }),
            KeyEventKind::Release => PlatformInput::KeyUp(KeyUpEvent { keystroke }),
        })
    }

    /// A mouse button, movement, or wheel step.
    pub fn mouse(&mut self, event: MouseEvent, mapping: &Mapping, now: Instant) -> PlatformInput {
        let position = mapping.position(event.column, event.row);
        self.position = position;
        // The terminal reports shift, alt, and control with the mouse; Cmd
        // comes from the keyboard.
        let modifiers = Modifiers {
            shift: event.modifiers.contains(KeyModifiers::SHIFT),
            control: event.modifiers.contains(KeyModifiers::CONTROL),
            alt: event.modifiers.contains(KeyModifiers::ALT),
            platform: self.modifiers.platform,
            function: false,
        };
        let step = mapping.wheel_step();
        let wheel = |x: Pixels, y: Pixels| {
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(x, y)),
                modifiers,
                touch_phase: TouchPhase::Moved,
            })
        };
        match event.kind {
            MouseEventKind::Down(button) => {
                let button = gpui_button(button);
                PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position,
                    modifiers,
                    click_count: self.click(button, position, now),
                    first_mouse: false,
                })
            }
            MouseEventKind::Up(button) => {
                let button = gpui_button(button);
                PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position,
                    modifiers,
                    click_count: self
                        .last_click
                        .as_ref()
                        .filter(|click| click.button == button)
                        .map_or(1, |click| click.count),
                })
            }
            MouseEventKind::Drag(button) => PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: Some(gpui_button(button)),
                modifiers,
            }),
            MouseEventKind::Moved => PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }),
            MouseEventKind::ScrollUp => wheel(px(0.), step),
            MouseEventKind::ScrollDown => wheel(px(0.), -step),
            MouseEventKind::ScrollLeft => wheel(step, px(0.)),
            MouseEventKind::ScrollRight => wheel(-step, px(0.)),
        }
    }

    /// The click count for a press: one more than the last press when it was
    /// the same button, close by, and soon enough.
    fn click(&mut self, button: MouseButton, position: Point<Pixels>, now: Instant) -> usize {
        let count = match &self.last_click {
            Some(last)
                if last.button == button
                    && now.duration_since(last.at) <= MULTI_CLICK_TIME
                    && (last.position.x - position.x).abs() <= px(MULTI_CLICK_DISTANCE)
                    && (last.position.y - position.y).abs() <= px(MULTI_CLICK_DISTANCE) =>
            {
                last.count + 1
            }
            _ => 1,
        };
        self.last_click = Some(Click {
            button,
            at: now,
            position,
            count,
        });
        count
    }
}

fn gpui_button(button: TermButton) -> MouseButton {
    match button {
        TermButton::Left => MouseButton::Left,
        TermButton::Right => MouseButton::Right,
        TermButton::Middle => MouseButton::Middle,
    }
}

/// The keystroke macOS would report for this key, so key bindings match the
/// same way they do in the window: letters keep shift (`shift-a`), other
/// shifted keys are the character they type (`>`), and only keys without
/// Ctrl or Cmd carry the text they'd insert.
pub fn keystroke(code: KeyCode, held: KeyModifiers) -> Option<Keystroke> {
    let mut modifiers = Modifiers {
        control: held.contains(KeyModifiers::CONTROL),
        alt: held.contains(KeyModifiers::ALT),
        shift: held.contains(KeyModifiers::SHIFT),
        platform: held.contains(KeyModifiers::SUPER),
        function: false,
    };
    let named = |key: &str| (key.to_string(), None);
    let (key, key_char) = match code {
        KeyCode::Char(' ') => ("space".to_string(), Some(" ".to_string())),
        KeyCode::Char(c) => {
            let lower: String = c.to_lowercase().collect();
            let key = if c.is_uppercase() && lower != c.to_string() {
                modifiers.shift = true;
                lower
            } else {
                c.to_string()
            };
            let typed = !modifiers.control && !modifiers.platform;
            (key, typed.then(|| c.to_string()))
        }
        KeyCode::Enter => ("enter".to_string(), Some("\n".to_string())),
        KeyCode::Tab => ("tab".to_string(), Some("\t".to_string())),
        KeyCode::BackTab => {
            modifiers.shift = true;
            named("tab")
        }
        KeyCode::Backspace => named("backspace"),
        KeyCode::Esc => named("escape"),
        KeyCode::Delete => named("delete"),
        KeyCode::Insert => named("insert"),
        KeyCode::Home => named("home"),
        KeyCode::End => named("end"),
        KeyCode::PageUp => named("pageup"),
        KeyCode::PageDown => named("pagedown"),
        KeyCode::Up => named("up"),
        KeyCode::Down => named("down"),
        KeyCode::Left => named("left"),
        KeyCode::Right => named("right"),
        KeyCode::F(number) => (format!("f{number}"), None),
        KeyCode::Null
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => return None,
    };
    Some(Keystroke {
        modifiers,
        key,
        key_char,
    })
}

/// The text a key press types into a text field, if it isn't claimed by a
/// key binding: its character, with no modifier but shift.
pub fn typed_text(input: &PlatformInput) -> Option<&str> {
    match input {
        PlatformInput::KeyDown(event)
            if event.keystroke.modifiers.is_subset_of(&Modifiers::shift()) =>
        {
            event.keystroke.key_char.as_deref()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::empty(),
        }
    }

    fn unparse(code: KeyCode, modifiers: KeyModifiers) -> String {
        keystroke(code, modifiers)
            .map(|k| k.unparse())
            .unwrap_or_default()
    }

    #[test]
    fn keystrokes_follow_macos() {
        let none = KeyModifiers::NONE;
        assert_eq!(unparse(KeyCode::Char('a'), none), "a");
        assert_eq!(unparse(KeyCode::Char('A'), none), "shift-a");
        assert_eq!(unparse(KeyCode::Char('>'), none), ">");
        assert_eq!(unparse(KeyCode::Char('c'), KeyModifiers::CONTROL), "ctrl-c");
        // gpui spells the platform modifier per OS: cmd, super, or win.
        let platform_shift_q = Keystroke::parse("cmd-shift-q")
            .expect("a valid keystroke")
            .unparse();
        assert_eq!(
            unparse(KeyCode::Char('Q'), KeyModifiers::SUPER),
            platform_shift_q
        );
        assert_eq!(unparse(KeyCode::Char(' '), none), "space");
        assert_eq!(unparse(KeyCode::BackTab, none), "shift-tab");
        assert_eq!(unparse(KeyCode::Up, KeyModifiers::ALT), "alt-up");
        assert_eq!(unparse(KeyCode::F(2), none), "f2");
        assert_eq!(unparse(KeyCode::CapsLock, none), "");
    }

    #[test]
    fn only_plain_keys_carry_text() {
        let text = |code, modifiers| keystroke(code, modifiers).and_then(|k| k.key_char);
        assert_eq!(
            text(KeyCode::Char('é'), KeyModifiers::NONE),
            Some("é".into())
        );
        assert_eq!(text(KeyCode::Char('a'), KeyModifiers::CONTROL), None);
        assert_eq!(text(KeyCode::Char('a'), KeyModifiers::SUPER), None);
        assert_eq!(text(KeyCode::Enter, KeyModifiers::NONE), Some("\n".into()));
        assert_eq!(text(KeyCode::Backspace, KeyModifiers::NONE), None);

        let mut translator = Translator::default();
        let typed = translator
            .key(key(
                KeyCode::Char('A'),
                KeyModifiers::NONE,
                KeyEventKind::Press,
            ))
            .unwrap();
        assert_eq!(typed_text(&typed), Some("A"));
        let shortcut = translator
            .key(key(
                KeyCode::Char('a'),
                KeyModifiers::ALT,
                KeyEventKind::Press,
            ))
            .unwrap();
        assert_eq!(typed_text(&shortcut), None);
    }

    #[test]
    fn presses_repeats_and_releases() {
        let mut translator = Translator::default();
        let mut kind = |kind| translator.key(key(KeyCode::Down, KeyModifiers::NONE, kind));
        assert!(matches!(
            kind(KeyEventKind::Press),
            Some(PlatformInput::KeyDown(KeyDownEvent { is_held: false, .. }))
        ));
        assert!(matches!(
            kind(KeyEventKind::Repeat),
            Some(PlatformInput::KeyDown(KeyDownEvent { is_held: true, .. }))
        ));
        assert!(matches!(
            kind(KeyEventKind::Release),
            Some(PlatformInput::KeyUp(_))
        ));
    }

    #[test]
    fn modifier_keys_are_tracked_for_the_mouse() {
        let mut translator = Translator::default();
        let cmd = KeyCode::Modifier(ModifierKeyCode::LeftSuper);
        // crossterm reports the modifier as held on release too.
        let pressed = translator.key(key(cmd, KeyModifiers::SUPER, KeyEventKind::Press));
        assert!(matches!(
            pressed,
            Some(PlatformInput::ModifiersChanged(ModifiersChangedEvent { modifiers, .. }))
                if modifiers.platform
        ));
        let mapping = Mapping {
            scale: 2.,
            cell: (16., 32.),
            pixels: true,
        };
        let click = MouseEvent {
            kind: MouseEventKind::Down(TermButton::Left),
            column: 99,
            row: 199,
            modifiers: KeyModifiers::NONE,
        };
        let PlatformInput::MouseDown(down) = translator.mouse(click, &mapping, Instant::now())
        else {
            panic!("expected a mouse down");
        };
        assert!(down.modifiers.platform);
        assert_eq!(down.position, point(px(50.), px(100.)));

        translator.key(key(cmd, KeyModifiers::SUPER, KeyEventKind::Release));
        assert!(!translator.modifiers().platform);
    }

    #[test]
    fn positions_from_pixels_or_cells() {
        let pixels = Mapping {
            scale: 2.,
            cell: (16., 32.),
            pixels: true,
        };
        assert_eq!(pixels.position(u16::MAX, 9), point(px(0.), px(5.)));
        let cells = Mapping {
            pixels: false,
            ..pixels
        };
        assert_eq!(cells.position(0, 1), point(px(4.), px(24.)));
    }

    #[test]
    fn quick_clicks_in_place_count_up() {
        let mut translator = Translator::default();
        let mapping = Mapping {
            scale: 1.,
            cell: (8., 16.),
            pixels: true,
        };
        let start = Instant::now();
        let mut press = |column, at| {
            let event = MouseEvent {
                kind: MouseEventKind::Down(TermButton::Left),
                column,
                row: 10,
                modifiers: KeyModifiers::NONE,
            };
            match translator.mouse(event, &mapping, at) {
                PlatformInput::MouseDown(down) => down.click_count,
                _ => 0,
            }
        };
        assert_eq!(press(10, start), 1);
        assert_eq!(press(11, start + Duration::from_millis(200)), 2);
        assert_eq!(press(12, start + Duration::from_millis(400)), 3);
        assert_eq!(
            press(40, start + Duration::from_millis(500)),
            1,
            "moved away"
        );
        assert_eq!(press(40, start + Duration::from_secs(2)), 1, "too slow");
    }

    #[test]
    fn wheel_steps_are_a_cell_high() {
        let mut translator = Translator::default();
        let mapping = Mapping {
            scale: 2.,
            cell: (16., 34.),
            pixels: true,
        };
        let event = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        let PlatformInput::ScrollWheel(ScrollWheelEvent {
            delta: ScrollDelta::Pixels(delta),
            ..
        }) = translator.mouse(event, &mapping, Instant::now())
        else {
            panic!("expected a wheel event in pixels");
        };
        assert_eq!(delta, point(px(0.), px(-17.)));
    }
}
