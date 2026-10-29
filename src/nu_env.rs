//! The user's Nushell environment, for the option to use its file colors
//! (`$env.LS_COLORS`) and editor (`$env.config.buffer_editor`). Read once
//! from a login shell, so config.nu and env.nu apply.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fffinder_core::FileEntry;
#[cfg(test)]
use gpui_kit::Rgba;
use gpui_kit::{
    FontStyle, FontWeight, HighlightStyle, Hsla, StrikethroughStyle, UnderlineStyle, px,
};

use crate::ansi::xterm_rgb;
use lscolors::{Color, Indicator, LsColors};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NuEnv {
    pub ls_colors: Option<String>,
    /// A command name, or a list of the command and its arguments.
    pub buffer_editor: Option<serde_json::Value>,
    pub visual: Option<String>,
    pub editor: Option<String>,
    #[serde(default)]
    pub path: Vec<String>,
}

/// Read the environment of `nu --login`. Login configs may print, so the
/// values go to a file instead of stdout.
pub fn load(nu: &Path) -> Result<NuEnv, String> {
    let out = std::env::temp_dir().join(format!(
        "fffinder-nu-env-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let script = format!(
        "{{\
            ls_colors: ($env.LS_COLORS? | default null), \
            buffer_editor: ($env.config.buffer_editor? | default null), \
            visual: ($env.VISUAL? | default null), \
            editor: ($env.EDITOR? | default null), \
            path: ($env.PATH? | default $env.Path? | default []) \
        }} | to json | save --force {}",
        serde_json::to_string(&out.to_string_lossy()).unwrap_or_default()
    );
    let mut child = std::process::Command::new(nu)
        .args(crate::nushell::login_args())
        .args(["-c", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", nu.display()))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return Err("nu --login took more than 15 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let text = std::fs::read_to_string(&out);
    let _ = std::fs::remove_file(&out);
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            let _ = stderr.read_to_string(&mut err);
        }
        return Err(format!("nu --login failed: {}", err.trim()));
    }
    let text = text.map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Editors that need a terminal of their own.
const TERMINAL_EDITORS: &[&str] = &[
    "vi", "vim", "nvim", "lvim", "hx", "helix", "nano", "pico", "micro", "kak", "joe", "ne", "mg",
    "emacs", "amp", "ed", "vis", "jed", "mcedit",
];

impl NuEnv {
    /// The editor command: `buffer_editor`, then `$VISUAL`, then `$EDITOR`.
    pub fn editor(&self) -> Option<Vec<String>> {
        let from_config = match &self.buffer_editor {
            Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Some(vec![s.clone()]),
            Some(serde_json::Value::Array(words)) => {
                let words: Vec<String> = words
                    .iter()
                    .filter_map(|w| w.as_str().map(str::to_string))
                    .collect();
                (!words.is_empty()).then_some(words)
            }
            _ => None,
        };
        from_config.or_else(|| {
            [&self.visual, &self.editor]
                .into_iter()
                .flatten()
                .find(|e| !e.trim().is_empty())
                .map(|e| e.split_whitespace().map(str::to_string).collect())
        })
    }

    /// `program` found on the shell's `PATH` (GUI apps get a short one).
    pub fn resolve(&self, program: &str) -> PathBuf {
        let path = Path::new(program);
        if path.is_absolute() || program.contains(std::path::MAIN_SEPARATOR) {
            return path.to_path_buf();
        }
        let exe = if cfg!(windows) && path.extension().is_none() {
            format!("{program}.exe")
        } else {
            program.to_string()
        };
        self.path
            .iter()
            .map(|dir| Path::new(dir).join(&exe))
            .find(|p| p.is_file())
            .unwrap_or_else(|| path.to_path_buf())
    }

    /// The colors `ls` uses in Nushell: `LS_COLORS`, or Nushell's own
    /// defaults when it isn't set.
    pub fn ls_colors(&self) -> LsColors {
        match &self.ls_colors {
            Some(s) if !s.is_empty() => LsColors::from_string(s),
            _ => LsColors::from_string(NU_DEFAULT_LS_COLORS),
        }
    }
}

/// Whether `program` (a path or name) is an editor that runs in a terminal.
pub fn needs_terminal(program: &str) -> bool {
    let name = Path::new(program)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    TERMINAL_EDITORS.contains(&name.as_str())
}

/// How `ls` draws a name: `LS_COLORS`' colors and attributes. Names like
/// README get a background, so it is drawn behind the letters as in a
/// terminal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NameStyle {
    pub color: Option<Hsla>,
    pub background: Option<Hsla>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub dim: bool,
}

impl NameStyle {
    pub fn highlight(&self) -> HighlightStyle {
        HighlightStyle {
            color: self.color,
            background_color: self.background,
            font_weight: self.bold.then_some(FontWeight::BOLD),
            font_style: self.italic.then_some(FontStyle::Italic),
            underline: self.underline.then(|| UnderlineStyle {
                thickness: px(1.),
                ..Default::default()
            }),
            strikethrough: self.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.),
                ..Default::default()
            }),
            fade_out: self.dim.then_some(0.35),
        }
    }
}

/// How `ls` would draw `entry`'s name, from what the window already knows
/// about it (no extra disk reads); `None` when it's plain.
pub fn name_style(colors: &LsColors, entry: &FileEntry) -> Option<NameStyle> {
    let indicator = if entry.is_symlink {
        Indicator::SymbolicLink
    } else if entry.is_dir {
        Indicator::Directory
    } else if entry.kind == fffinder_core::FileKind::Executable {
        Indicator::ExecutableFile
    } else {
        Indicator::RegularFile
    };
    let style = if indicator == Indicator::RegularFile {
        colors
            .style_for_str(&entry.name)
            .or_else(|| colors.style_for_indicator(indicator))
    } else {
        colors.style_for_indicator(indicator)
    }?;
    let font = &style.font_style;
    let (mut color, mut background) = (
        style.foreground.as_ref().map(to_hsla),
        style.background.as_ref().map(to_hsla),
    );
    if font.reverse {
        std::mem::swap(&mut color, &mut background);
    }
    let name = NameStyle {
        color,
        background,
        bold: font.bold,
        italic: font.italic,
        underline: font.underline,
        strikethrough: font.strikethrough,
        dim: font.dimmed,
    };
    (name.highlight() != HighlightStyle::default()).then_some(name)
}

fn to_hsla(color: &Color) -> Hsla {
    let (r, g, b) = match color {
        Color::RGB(r, g, b) => (*r, *g, *b),
        Color::Fixed(n) => xterm_rgb(*n),
        named => xterm_rgb(match named {
            Color::Black => 0,
            Color::Red => 1,
            Color::Green => 2,
            Color::Yellow => 3,
            Color::Blue => 4,
            Color::Magenta => 5,
            Color::Cyan => 6,
            Color::White => 7,
            Color::BrightBlack => 8,
            Color::BrightRed => 9,
            Color::BrightGreen => 10,
            Color::BrightYellow => 11,
            Color::BrightBlue => 12,
            Color::BrightMagenta => 13,
            Color::BrightCyan => 14,
            _ => 15,
        }),
    };
    crate::ansi::rgb(r, g, b)
}

/// Nushell's colors for `ls` when `LS_COLORS` isn't set
/// (`nu_utils::get_ls_colors`).
const NU_DEFAULT_LS_COLORS: &str = "*~=0;38;5;243:bd=0;38;5;81;48;5;236:ca=0:cd=0;38;5;203;48;5;236:di=0;38;5;81:do=0;38;5;16;48;5;203:ex=1;38;5;203:fi=0:ln=0;38;5;203:mh=0:mi=0;38;5;16;48;5;203:no=0:or=0;38;5;16;48;5;203:ow=0:pi=0;38;5;16;48;5;81:rs=0:sg=0:so=0;38;5;16;48;5;203:st=0:su=0:tw=0:*.1=0;38;5;185:*.a=1;38;5;203:*.c=0;38;5;48:*.d=0;38;5;48:*.f=0;38;5;48:*.h=0;38;5;48:*.m=0;38;5;48:*.o=0;38;5;243:*.p=0;38;5;48:*.r=0;38;5;48:*.t=0;38;5;48:*.v=0;38;5;48:*.z=4;38;5;203:*.7z=4;38;5;203:*.ai=0;38;5;208:*.as=0;38;5;48:*.au=0;38;5;208:*.bc=0;38;5;243:*.br=4;38;5;203:*.bz=4;38;5;203:*.cc=0;38;5;48:*.cp=0;38;5;48:*.cr=0;38;5;48:*.cs=0;38;5;48:*.db=4;38;5;203:*.di=0;38;5;48:*.el=0;38;5;48:*.ex=0;38;5;48:*.fs=0;38;5;48:*.go=0;38;5;48:*.gv=0;38;5;48:*.gz=4;38;5;203:*.ha=0;38;5;48:*.hh=0;38;5;48:*.hi=0;38;5;243:*.hs=0;38;5;48:*.jl=0;38;5;48:*.js=0;38;5;48:*.ko=1;38;5;203:*.kt=0;38;5;48:*.la=0;38;5;243:*.ll=0;38;5;48:*.lo=0;38;5;243:*.lz=4;38;5;203:*.ma=0;38;5;208:*.mb=0;38;5;208:*.md=0;38;5;185:*.mk=0;38;5;149:*.ml=0;38;5;48:*.mn=0;38;5;48:*.nb=0;38;5;48:*.nu=0;38;5;48:*.pl=0;38;5;48:*.pm=0;38;5;48:*.pp=0;38;5;48:*.ps=0;38;5;186:*.py=0;38;5;48:*.rb=0;38;5;48:*.rc=0;38;5;149:*.rm=0;38;5;208:*.rs=0;38;5;48:*.sh=0;38;5;48:*.so=1;38;5;203:*.sz=4;38;5;203:*.td=0;38;5;48:*.tf=0;38;5;48:*.ts=0;38;5;48:*.ui=0;38;5;149:*.vb=0;38;5;48:*.wv=0;38;5;208:*.xz=4;38;5;203:*.zz=4;38;5;203:*FAQ=0;38;5;16;48;5;186:*.3ds=0;38;5;208:*.3fr=0;38;5;208:*.3g2=0;38;5;208:*.3ga=0;38;5;208:*.3gp=0;38;5;208:*.3mf=0;38;5;208:*.aac=0;38;5;208:*.adb=0;38;5;48:*.ads=0;38;5;48:*.afm=0;38;5;208:*.aif=0;38;5;208:*.amf=0;38;5;208:*.amr=0;38;5;208:*.ape=0;38;5;208:*.apk=4;38;5;203:*.ari=0;38;5;208:*.arj=4;38;5;203:*.arw=0;38;5;208:*.asa=0;38;5;48:*.ase=0;38;5;208:*.asf=0;38;5;208:*.asm=0;38;5;48:*.aux=0;38;5;243:*.avi=0;38;5;208:*.awk=0;38;5;48:*.azw=0;38;5;186:*.bag=4;38;5;203:*.bak=0;38;5;243:*.bat=1;38;5;203:*.bay=0;38;5;208:*.bbl=0;38;5;243:*.bcf=0;38;5;243:*.bib=0;38;5;149:*.bin=4;38;5;203:*.blg=0;38;5;243:*.bmp=0;38;5;208:*.bsh=0;38;5;48:*.bsp=4;38;5;203:*.bst=0;38;5;149:*.bz2=4;38;5;203:*.bzl=0;38;5;149:*.c++=0;38;5;48:*.cab=4;38;5;203:*.caf=0;38;5;208:*.cap=0;38;5;208:*.cbr=0;38;5;186:*.cbz=0;38;5;186:*.cda=0;38;5;208:*.cfg=0;38;5;149:*.cgi=0;38;5;48:*.cjs=0;38;5;48:*.clj=0;38;5;48:*.com=1;38;5;203:*.cpp=0;38;5;48:*.cr2=0;38;5;208:*.cr3=0;38;5;208:*.crw=0;38;5;208:*.crx=4;38;5;203:*.css=0;38;5;48:*.csv=0;38;5;186:*.csx=0;38;5;48:*.cxx=0;38;5;48:*.dae=0;38;5;208:*.dat=0;38;5;208:*.dcr=0;38;5;208:*.dcs=0;38;5;208:*.deb=4;38;5;203:*.def=0;38;5;48:*.dll=1;38;5;203:*.dmg=4;38;5;203:*.dmp=0;38;5;243:*.dng=0;38;5;208:*.doc=0;38;5;186:*.dot=0;38;5;48:*.dox=0;38;5;149:*.dpr=0;38;5;48:*.drf=0;38;5;208:*.dts=0;38;5;208:*.dwg=0;38;5;208:*.dxf=0;38;5;208:*.ear=4;38;5;203:*.eip=0;38;5;208:*.elc=0;38;5;48:*.elm=0;38;5;48:*.eln=0;38;5;48:*.eml=0;38;5;186:*.epp=0;38;5;48:*.eps=0;38;5;208:*.erf=0;38;5;208:*.erl=0;38;5;48:*.etx=0;38;5;185:*.exe=1;38;5;203:*.exr=0;38;5;208:*.exs=0;38;5;48:*.f4v=0;38;5;208:*.fb2=0;38;5;186:*.fbx=0;38;5;208:*.fcm=0;38;5;208:*.fff=0;38;5;208:*.fls=0;38;5;243:*.flv=0;38;5;208:*.fnt=0;38;5;208:*.fon=0;38;5;208:*.for=0;38;5;48:*.fsi=0;38;5;48:*.fsx=0;38;5;48:*.fvd=4;38;5;203:*.gif=0;38;5;208:*.git=0;38;5;243:*.gp3=0;38;5;208:*.gp4=0;38;5;208:*.gpr=0;38;5;208:*.gvy=0;38;5;48:*.h++=0;38;5;48:*.hda=0;38;5;208:*.hip=0;38;5;208:*.hpp=0;38;5;48:*.htc=0;38;5;48:*.htm=0;38;5;185:*.hxx=0;38;5;48:*.ico=0;38;5;208:*.ics=0;38;5;186:*.idx=0;38;5;243:*.igs=0;38;5;208:*.iiq=0;38;5;208:*.ilg=0;38;5;243:*.img=4;38;5;203:*.inc=0;38;5;48:*.ind=0;38;5;243:*.ini=0;38;5;149:*.inl=0;38;5;48:*.ino=0;38;5;48:*.ipa=4;38;5;203:*.ipp=0;38;5;48:*.iso=4;38;5;203:*.jad=4;38;5;203:*.jar=4;38;5;203:*.jpg=0;38;5;208:*.jsx=0;38;5;48:*.jxl=0;38;5;208:*.k25=0;38;5;208:*.kdc=0;38;5;208:*.key=0;38;5;186:*.kra=0;38;5;208:*.kts=0;38;5;48:*.lhs=0;38;5;48:*.log=0;38;5;243:*.lrz=4;38;5;203:*.ltx=0;38;5;48:*.lua=0;38;5;48:*.lzo=4;38;5;203:*.m2v=0;38;5;208:*.m3u=0;38;5;208:*.m4a=0;38;5;208:*.m4v=0;38;5;208:*.mdc=0;38;5;208:*.mdx=0;38;5;185:*.mef=0;38;5;208:*.mht=4;38;5;203:*.mid=0;38;5;208:*.mir=0;38;5;48:*.mjs=0;38;5;48:*.mkd=0;38;5;185:*.mkv=0;38;5;208:*.mli=0;38;5;48:*.mly=0;38;5;48:*.mod=0;38;5;208:*.mos=0;38;5;208:*.mov=0;38;5;208:*.mp3=0;38;5;208:*.mp4=0;38;5;208:*.mpg=0;38;5;208:*.mrw=0;38;5;208:*.msg=0;38;5;186:*.msi=4;38;5;203:*.mtl=0;38;5;208:*.nef=0;38;5;208:*.nfo=0;38;5;185:*.nim=0;38;5;48:*.nix=0;38;5;149:*.nrg=4;38;5;203:*.nrw=0;38;5;208:*.obj=0;38;5;208:*.obm=0;38;5;208:*.odp=0;38;5;186:*.ods=0;38;5;186:*.odt=0;38;5;186:*.oga=0;38;5;208:*.ogg=0;38;5;208:*.ogm=0;38;5;208:*.ogv=0;38;5;208:*.orf=0;38;5;208:*.org=0;38;5;185:*.otf=0;38;5;208:*.otl=0;38;5;208:*.out=0;38;5;243:*.pak=4;38;5;203:*.pas=0;38;5;48:*.pbm=0;38;5;208:*.pcm=0;38;5;208:*.pcx=0;38;5;208:*.pdf=0;38;5;186:*.pef=0;38;5;208:*.pfa=0;38;5;208:*.pfb=0;38;5;208:*.pfm=0;38;5;208:*.pgm=0;38;5;208:*.php=0;38;5;48:*.pid=0;38;5;243:*.pk3=4;38;5;203:*.pkg=4;38;5;203:*.ply=0;38;5;208:*.png=0;38;5;208:*.pod=0;38;5;48:*.ppm=0;38;5;208:*.pps=0;38;5;186:*.ppt=0;38;5;186:*.pro=0;38;5;149:*.ps1=0;38;5;48:*.psd=0;38;5;208:*.ptx=0;38;5;208:*.pxd=0;38;5;208:*.pxm=0;38;5;208:*.pxn=0;38;5;208:*.pyc=0;38;5;243:*.pyd=0;38;5;243:*.pyo=0;38;5;243:*.qoi=0;38;5;208:*.r3d=0;38;5;208:*.raf=0;38;5;208:*.rar=4;38;5;203:*.raw=0;38;5;208:*.rpm=4;38;5;203:*.rss=0;38;5;185:*.rst=0;38;5;185:*.rtf=0;38;5;186:*.rw2=0;38;5;208:*.rwl=0;38;5;208:*.rwz=0;38;5;208:*.s3m=0;38;5;208:*.s7z=4;38;5;203:*.sbt=0;38;5;48:*.sed=0;38;5;48:*.sid=0;38;5;208:*.sql=0;38;5;48:*.sr2=0;38;5;208:*.srf=0;38;5;208:*.srw=0;38;5;208:*.stl=0;38;5;208:*.stp=0;38;5;208:*.sty=0;38;5;243:*.svg=0;38;5;208:*.swf=0;38;5;208:*.swn=0;38;5;243:*.swo=0;38;5;243:*.swp=0;38;5;243:*.sxi=0;38;5;186:*.sxw=0;38;5;186:*.tar=4;38;5;203:*.tbz=4;38;5;203:*.tcl=0;38;5;48:*.tex=0;38;5;48:*.tga=0;38;5;208:*.tgz=4;38;5;203:*.tif=0;38;5;208:*.tml=0;38;5;149:*.tmp=0;38;5;243:*.toc=0;38;5;243:*.tsv=0;38;5;186:*.tsx=0;38;5;48:*.ttf=0;38;5;208:*.txt=0;38;5;185:*.typ=0;38;5;185:*.usd=0;38;5;208:*.vba=0;38;5;48:*.vbs=0;38;5;48:*.vcd=4;38;5;203:*.vcf=0;38;5;186:*.vdf=4;38;5;203:*.vdi=4;38;5;203:*.vfd=4;38;5;203:*.vhd=4;38;5;203:*.vim=0;38;5;48:*.vob=0;38;5;208:*.vpk=4;38;5;203:*.vsh=0;38;5;48:*.war=4;38;5;203:*.wav=0;38;5;208:*.wma=0;38;5;208:*.wmv=0;38;5;208:*.wrl=0;38;5;208:*.wvc=0;38;5;208:*.x3d=0;38;5;208:*.x3f=0;38;5;208:*.xcf=0;38;5;208:*.xla=0;38;5;186:*.xlr=0;38;5;186:*.xls=0;38;5;186:*.xlt=0;38;5;186:*.xml=0;38;5;185:*.xmp=0;38;5;149:*.xpi=4;38;5;203:*.xpm=0;38;5;208:*.xsd=0;38;5;185:*.xsh=0;38;5;48:*.yml=0;38;5;149:*.zig=0;38;5;48:*.zip=4;38;5;203:*.zoo=4;38;5;203:*.zsh=0;38;5;48:*.zst=4;38;5;203:*TODO=1:*cljc=0;38;5;48:*hgrc=0;38;5;149:*mtab=0;38;5;149:*.adoc=0;38;5;185:*.agda=0;38;5;48:*.aiff=0;38;5;208:*.alac=0;38;5;208:*.avif=0;38;5;208:*.azw3=0;38;5;186:*.bash=0;38;5;48:*.braw=0;38;5;208:*.cljs=0;38;5;48:*.cljw=0;38;5;48:*.conf=0;38;5;149:*.cpio=4;38;5;203:*.dart=0;38;5;48:*.data=0;38;5;208:*.diff=0;38;5;48:*.divx=0;38;5;208:*.djvu=0;38;5;186:*.docm=0;38;5;186:*.docx=0;38;5;186:*.dump=0;38;5;243:*.epsf=0;38;5;208:*.epub=0;38;5;186:*.fish=0;38;5;48:*.flac=0;38;5;208:*.fxml=0;38;5;185:*.h264=0;38;5;208:*.hack=0;38;5;48:*.heif=0;38;5;208:*.hgrc=0;38;5;149:*.html=0;38;5;185:*.icns=0;38;5;208:*.iges=0;38;5;208:*.info=0;38;5;185:*.java=0;38;5;48:*.jhtm=0;38;5;185:*.jpeg=0;38;5;208:*.json=0;38;5;149:*.less=0;38;5;48:*.lisp=0;38;5;48:*.lock=0;38;5;243:*.lzma=4;38;5;203:*.make=0;38;5;149:*.midi=0;38;5;208:*.mobi=0;38;5;186:*.mojo=0;38;5;48:*.mp4a=0;38;5;208:*.mpeg=0;38;5;208:*.nims=0;38;5;48:*.norg=0;38;5;185:*.odin=0;38;5;48:*.opus=0;38;5;208:*.orig=0;38;5;243:*.part=0;38;5;243:*.path=0;38;5;149:*.pcap=0;38;5;243:*.ppts=0;38;5;186:*.pptx=0;38;5;186:*.prql=0;38;5;48:*.psd1=0;38;5;48:*.psm1=0;38;5;48:*.purs=0;38;5;48:*.qcow=4;38;5;203:*.rake=0;38;5;149:*.raku=0;38;5;48:*.rlib=0;38;5;243:*.rmvb=0;38;5;208:*.sass=0;38;5;48:*.scad=0;38;5;48:*.scss=0;38;5;48:*.step=0;38;5;208:*.swap=0;38;5;149:*.tbz2=4;38;5;203:*.tiff=0;38;5;208:*.toml=0;38;5;149:*.usda=0;38;5;208:*.usdc=0;38;5;208:*.usdz=0;38;5;208:*.vala=0;38;5;48:*.vapi=0;38;5;48:*.vhdx=4;38;5;203:*.vmdk=4;38;5;203:*.warc=4;38;5;203:*.webm=0;38;5;208:*.webp=0;38;5;208:*.woff=0;38;5;208:*.xbps=4;38;5;203:*.xlsm=0;38;5;186:*.xlsx=0;38;5;186:*.xltm=0;38;5;186:*.yaml=0;38;5;149:*.zipx=4;38;5;203:*.zpaq=4;38;5;203:*.zstd=4;38;5;203:*fstab=0;38;5;149:*group=0;38;5;149:*hosts=0;38;5;149:*stdin=0;38;5;243:*v.mod=0;38;5;149:*.aria2=0;38;5;243:*.bazel=0;38;5;149:*.blend=0;38;5;208:*.cabal=0;38;5;48:*.cache=0;38;5;243:*.class=0;38;5;243:*.cmake=0;38;5;149:*.ctags=0;38;5;243:*.dylib=1;38;5;203:*.dyn_o=0;38;5;243:*.gcode=0;38;5;48:*.ipynb=0;38;5;48:*.jhtml=0;38;5;185:*.lagda=0;38;5;48:*.mdown=0;38;5;185:*.mhtml=4;38;5;203:*.mount=0;38;5;149:*.pages=0;38;5;186:*.paq8n=4;38;5;203:*.paq8o=4;38;5;203:*.patch=0;38;5;48:*.plist=0;38;5;243:*.pptsm=0;38;5;186:*.pptxm=0;38;5;186:*.rmeta=0;38;5;243:*.scala=0;38;5;48:*.shtml=0;38;5;185:*.state=0;38;5;243:*.swift=0;38;5;48:*.timer=0;38;5;149:*.toast=4;38;5;203:*.vcard=0;38;5;186:*.woff2=0;38;5;208:*.xhtml=0;38;5;185:*Icon\\r=0;38;5;243:*LEGACY=0;38;5;16;48;5;186:*NOTICE=0;38;5;16;48;5;186:*README=0;38;5;16;48;5;186:*go.mod=0;38;5;149:*go.sum=0;38;5;243:*mounts=0;38;5;149:*passwd=0;38;5;149:*shadow=0;38;5;149:*stderr=0;38;5;243:*stdout=0;38;5;243:*.bashrc=0;38;5;48:*.config=0;38;5;149:*.device=0;38;5;149:*.dyn_hi=0;38;5;243:*.flake8=0;38;5;149:*.github=0;38;5;243:*.gradle=0;38;5;48:*.groovy=0;38;5;48:*.ignore=0;38;5;149:*.matlab=0;38;5;48:*.nimble=0;38;5;48:*.sample=0;38;5;208:*.socket=0;38;5;149:*.target=0;38;5;149:*.tfvars=0;38;5;48:*AUTHORS=0;38;5;16;48;5;186:*CHANGES=0;38;5;16;48;5;186:*COPYING=0;38;5;249:*HISTORY=0;38;5;16;48;5;186:*INSTALL=0;38;5;16;48;5;186:*LICENCE=0;38;5;249:*LICENSE=0;38;5;249:*PATENTS=0;38;5;249:*TODO.md=1:*VERSION=0;38;5;16;48;5;186:*crontab=0;38;5;149:*uv.lock=0;38;5;243:*.alembic=0;38;5;208:*.desktop=0;38;5;149:*.gemspec=0;38;5;149:*.mailmap=0;38;5;149:*.numbers=0;38;5;186:*.profile=0;38;5;48:*.service=0;38;5;149:*.textile=0;38;5;185:*.tfstate=0;38;5;243:*Doxyfile=0;38;5;149:*Makefile=0;38;5;149:*TODO.txt=1:*crypttab=0;38;5;149:*grub.cfg=0;38;5;149:*hostname=0;38;5;149:*networks=0;38;5;149:*setup.py=0;38;5;149:*.DS_Store=0;38;5;243:*.asciidoc=0;38;5;185:*.aseprite=0;38;5;208:*.cmake.in=0;38;5;149:*.fdignore=0;38;5;149:*.kdevelop=0;38;5;149:*.lockfile=0;38;5;243:*.markdown=0;38;5;185:*.rgignore=0;38;5;149:*.snapshot=0;38;5;149:*.tfignore=0;38;5;149:*CHANGELOG=0;38;5;16;48;5;186:*COPYRIGHT=0;38;5;249:*README.md=0;38;5;16;48;5;186:*bun.lockb=0;38;5;243:*configure=0;38;5;149:*securetty=0;38;5;149:*.automount=0;38;5;149:*.gitconfig=0;38;5;149:*.gitignore=0;38;5;149:*.localized=0;38;5;243:*.scons_opt=0;38;5;243:*.terraform=0;38;5;48:*.timestamp=0;38;5;243:*CHANGES.md=0;38;5;16;48;5;186:*CODEOWNERS=0;38;5;149:*Dockerfile=0;38;5;149:*INSTALL.md=0;38;5;16;48;5;186:*README.txt=0;38;5;16;48;5;186:*SConscript=0;38;5;149:*SConstruct=0;38;5;149:*.cirrus.yml=0;38;5;186:*.gitmodules=0;38;5;149:*.synctex.gz=0;38;5;243:*.travis.yml=0;38;5;186:*CHANGES.txt=0;38;5;16;48;5;186:*INSTALL.txt=0;38;5;16;48;5;186:*LICENSE-MIT=0;38;5;249:*MANIFEST.in=0;38;5;149:*Makefile.am=0;38;5;149:*Makefile.in=0;38;5;243:*known_hosts=0;38;5;149:*poetry.lock=0;38;5;243:*.applescript=0;38;5;48:*.fdb_latexmk=0;38;5;243:*.sparseimage=4;38;5;203:*.webmanifest=0;38;5;149:*CHANGELOG.md=0;38;5;16;48;5;186:*CONTRIBUTING=0;38;5;16;48;5;186:*CONTRIBUTORS=0;38;5;16;48;5;186:*appveyor.yml=0;38;5;186:*configure.ac=0;38;5;149:*.bash_profile=0;38;5;48:*.clang-format=0;38;5;149:*.dockerignore=0;38;5;149:*.editorconfig=0;38;5;149:*CHANGELOG.txt=0;38;5;16;48;5;186:*Containerfile=0;38;5;149:*.aseprite-keys=0;38;5;185:*.gitattributes=0;38;5;149:*.gitlab-ci.yml=0;38;5;186:*CMakeCache.txt=0;38;5;243:*CMakeLists.txt=0;38;5;149:*LICENSE-APACHE=0;38;5;249:*pyproject.toml=0;38;5;149:*CODE_OF_CONDUCT=0;38;5;16;48;5;186:*CONTRIBUTING.md=0;38;5;16;48;5;186:*CONTRIBUTORS.md=0;38;5;16;48;5;186:*authorized_keys=0;38;5;149:*.containerignore=0;38;5;149:*.sconsign.dblite=0;38;5;243:*CONTRIBUTING.txt=0;38;5;16;48;5;186:*CONTRIBUTORS.txt=0;38;5;16;48;5;186:*requirements.txt=0;38;5;149:*.aseprite-brushes=0;38;5;185:*package-lock.json=0;38;5;243:*CODE_OF_CONDUCT.md=0;38;5;16;48;5;186:*.CFUserTextEncoding=0;38;5;243:*.aseprite-extension=4;38;5;203:*CODE_OF_CONDUCT.txt=0;38;5;16;48;5;186:*azure-pipelines.yml=0;38;5;186";

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            path: PathBuf::from("/x").join(name),
            name: name.into(),
            is_dir,
            size: None,
            modified: None,
            kind: fffinder_core::FileKind::Other,
            hidden: false,
            is_symlink: false,
            created: None,
            mode: None,
            last_opened: None,
            readonly: false,
        }
    }

    #[test]
    fn colors_names_like_ls() {
        let colors = NuEnv::default().ls_colors();
        let color =
            |colors: &LsColors, e: &FileEntry| name_style(colors, e).unwrap().color.unwrap();
        let blue = color(&colors, &entry("src", true));
        let rust = color(&colors, &entry("main.rs", false));
        assert_ne!(blue, rust);
        assert_eq!(
            Rgba::from(rust),
            Rgba::from(to_hsla(&Color::Fixed(48))),
            "*.rs is color 48 in Nushell's defaults"
        );
        let mine = NuEnv {
            ls_colors: Some("di=1;31:*.md=38;2;1;2;3".into()),
            ..Default::default()
        }
        .ls_colors();
        assert_eq!(
            Rgba::from(color(&mine, &entry("notes.md", false))),
            Rgba::from(to_hsla(&Color::RGB(1, 2, 3)))
        );
        assert!(name_style(&mine, &entry("plain.txt", false)).is_none());

        // README is black on light yellow in Nushell's defaults; both colors
        // come through, and archives are underlined.
        let readme = name_style(&colors, &entry("README.md", false)).unwrap();
        assert_eq!(
            Rgba::from(readme.color.unwrap()),
            Rgba::from(to_hsla(&Color::Fixed(16)))
        );
        assert_eq!(
            Rgba::from(readme.background.unwrap()),
            Rgba::from(to_hsla(&Color::Fixed(186)))
        );
        assert!(
            name_style(&colors, &entry("a.zip", false))
                .unwrap()
                .underline
        );
        assert!(name_style(&colors, &entry("run.exe", false)).unwrap().bold);
    }

    #[test]
    fn picks_the_editor() {
        let env = NuEnv {
            buffer_editor: Some(serde_json::json!(["code", "--wait"])),
            editor: Some("vi".into()),
            ..Default::default()
        };
        assert_eq!(env.editor().unwrap(), ["code", "--wait"]);
        let env = NuEnv {
            buffer_editor: Some(serde_json::json!("")),
            visual: Some("hx -c x".into()),
            ..Default::default()
        };
        assert_eq!(env.editor().unwrap(), ["hx", "-c", "x"]);
        assert!(needs_terminal("/opt/homebrew/bin/nvim"));
        assert!(!needs_terminal("code"));
    }

    /// Runs only where `nu` is installed.
    #[test]
    fn reads_a_login_environment() {
        let Some(nu) = crate::nushell::find_nu(None) else {
            return;
        };
        let env = load(&nu).unwrap();
        assert!(!env.path.is_empty(), "{env:?}");
    }
}
