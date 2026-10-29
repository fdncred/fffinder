//! Group By: which group each item falls in, and arranging items into
//! groups in order, as Finder's Use Groups does.

use crate::entry::FileEntry;
use chrono::{DateTime, Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::time::SystemTime;

/// What items are grouped by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKey {
    Name,
    #[default]
    Kind,
    Modified,
    Created,
    LastOpened,
    Size,
}

impl GroupKey {
    pub const ALL: [GroupKey; 6] = [
        GroupKey::Name,
        GroupKey::Kind,
        GroupKey::Modified,
        GroupKey::Created,
        GroupKey::LastOpened,
        GroupKey::Size,
    ];

    /// The name the Group By menu shows: "Date Modified".
    pub fn label(self) -> &'static str {
        match self {
            GroupKey::Name => "Name",
            GroupKey::Kind => "Kind",
            GroupKey::Modified => "Date Modified",
            GroupKey::Created => "Date Created",
            GroupKey::LastOpened => "Date Last Opened",
            GroupKey::Size => "Size",
        }
    }
}

/// The group an item falls in. Groups sort by `rank`, then `label`, so
/// "Today" comes before "Yesterday" and kinds come alphabetically.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupId {
    rank: (u8, i64),
    pub label: String,
}

impl GroupId {
    fn new(class: u8, order: i64, label: impl Into<String>) -> GroupId {
        GroupId {
            rank: (class, order),
            label: label.into(),
        }
    }
}

/// Today's date where the user is, for [`group_of`].
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// The group `entry` falls in when grouping by `key`, with dates counted
/// back from `today`.
pub fn group_of(entry: &FileEntry, key: GroupKey, today: NaiveDate) -> GroupId {
    let local = |time: Option<SystemTime>| time.map(|t| DateTime::<Local>::from(t).date_naive());
    match key {
        GroupKey::Name => name_group(&entry.name),
        GroupKey::Kind => GroupId::new(0, 0, entry.kind_label()),
        GroupKey::Modified => date_group(local(entry.modified), today),
        GroupKey::Created => date_group(local(entry.created), today),
        GroupKey::LastOpened => date_group(local(entry.last_opened), today),
        GroupKey::Size => size_group(entry.size, entry.is_dir),
    }
}

/// "0–9" first, then each letter, then "Other".
fn name_group(name: &str) -> GroupId {
    match name.chars().next() {
        Some(c) if c.is_numeric() => GroupId::new(0, 0, "0–9"),
        Some(c) if c.is_alphabetic() => GroupId::new(1, 0, c.to_uppercase().collect::<String>()),
        _ => GroupId::new(2, 0, "Other"),
    }
}

/// Newest first: Today, Yesterday, Previous 7 Days, Previous 30 Days, the
/// earlier months of this year, then earlier years. Dates after today
/// count as today.
fn date_group(date: Option<NaiveDate>, today: NaiveDate) -> GroupId {
    let Some(date) = date else {
        return GroupId::new(6, 0, "No Date");
    };
    match (today - date).num_days() {
        ..=0 => GroupId::new(0, 0, "Today"),
        1 => GroupId::new(1, 0, "Yesterday"),
        2..=7 => GroupId::new(2, 0, "Previous 7 Days"),
        8..=30 => GroupId::new(3, 0, "Previous 30 Days"),
        _ if date.year() == today.year() => {
            GroupId::new(4, -i64::from(date.month()), date.format("%B").to_string())
        }
        _ => GroupId::new(5, -i64::from(date.year()), date.year().to_string()),
    }
}

/// Biggest first, in the same decimal units sizes are shown in. Folders
/// without a size go last.
fn size_group(size: Option<u64>, is_dir: bool) -> GroupId {
    const KB: u64 = 1000;
    const MB: u64 = 1000 * KB;
    const GB: u64 = 1000 * MB;
    match size {
        Some(GB..) => GroupId::new(0, 0, "Huge (over 1 GB)"),
        Some(100_000_000..) => GroupId::new(1, 0, "Large (100 MB to 1 GB)"),
        Some(MB..) => GroupId::new(2, 0, "Medium (1 to 100 MB)"),
        Some(100_000..) => GroupId::new(3, 0, "Small (100 KB to 1 MB)"),
        Some(1..) => GroupId::new(4, 0, "Tiny (under 100 KB)"),
        Some(0) => GroupId::new(5, 0, "Empty"),
        None if is_dir => GroupId::new(6, 0, "Folders"),
        None => GroupId::new(7, 0, "Unknown"),
    }
}

/// A group of arranged items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSpan {
    pub label: String,
    /// How many items are in the group, shown or not.
    pub total: usize,
    pub collapsed: bool,
    /// Where its items are in the arranged list; empty when collapsed.
    pub range: Range<usize>,
}

/// Put `items` into groups, in group order, keeping each group's items in
/// the order they came in. An item deeper than the first level (`depth` >
/// 0, an open folder's contents) stays with the item before it. The items
/// of collapsed groups are left out.
pub fn arrange<T>(
    items: Vec<T>,
    depth: impl Fn(&T) -> usize,
    group: impl Fn(&T) -> GroupId,
    collapsed: impl Fn(&str) -> bool,
) -> (Vec<T>, Vec<GroupSpan>) {
    // Each top-level item and the deeper items after it move as one block.
    let mut blocks: Vec<(GroupId, Range<usize>)> = Vec::new();
    for (ix, item) in items.iter().enumerate() {
        match blocks.last_mut() {
            Some((_, range)) if depth(item) > 0 => range.end = ix + 1,
            _ => blocks.push((group(item), ix..ix + 1)),
        }
    }
    // `sort_by` is stable, so items keep their order within a group.
    blocks.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Moving items out by index without cloning them: `Option::take`
    // leaves `None` behind, like popping from a list in Python.
    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    let mut arranged = Vec::with_capacity(slots.len());
    let mut spans: Vec<GroupSpan> = Vec::new();
    for (id, range) in blocks {
        if spans.last().is_none_or(|span| span.label != id.label) {
            let at = arranged.len();
            spans.push(GroupSpan {
                collapsed: collapsed(&id.label),
                label: id.label,
                total: 0,
                range: at..at,
            });
        }
        let span = spans.last_mut().expect("a span was just pushed");
        span.total += 1;
        if !span.collapsed {
            arranged.extend(slots[range].iter_mut().filter_map(Option::take));
            span.range.end = arranged.len();
        }
    }
    (arranged, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("a valid date")
    }

    #[test]
    fn dates_group_back_from_today() {
        let today = day(2026, 10, 28);
        let label = |date| date_group(Some(date), today).label;
        assert_eq!(label(day(2026, 11, 2)), "Today", "a future date");
        assert_eq!(label(today), "Today");
        assert_eq!(label(day(2026, 10, 27)), "Yesterday");
        assert_eq!(label(day(2026, 10, 26)), "Previous 7 Days");
        assert_eq!(label(day(2026, 10, 21)), "Previous 7 Days");
        assert_eq!(label(day(2026, 10, 20)), "Previous 30 Days");
        assert_eq!(label(day(2026, 9, 28)), "Previous 30 Days");
        assert_eq!(label(day(2026, 9, 27)), "September");
        assert_eq!(label(day(2026, 1, 1)), "January");
        assert_eq!(label(day(2025, 12, 31)), "2025");
        assert_eq!(date_group(None, today).label, "No Date");
        // Early in January, last December is still within 30 days.
        assert_eq!(
            date_group(Some(day(2025, 12, 20)), day(2026, 1, 5)).label,
            "Previous 30 Days"
        );

        let mut groups: Vec<GroupId> = [
            day(2024, 5, 1),
            day(2026, 3, 1),
            today,
            day(2025, 1, 1),
            day(2026, 7, 1),
            day(2026, 10, 27),
        ]
        .into_iter()
        .map(|d| date_group(Some(d), today))
        .chain([date_group(None, today)])
        .collect();
        groups.sort();
        let labels: Vec<&str> = groups.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Today",
                "Yesterday",
                "July",
                "March",
                "2025",
                "2024",
                "No Date"
            ]
        );
    }

    #[test]
    fn names_and_sizes_group() {
        assert_eq!(name_group("2026 taxes").label, "0–9");
        assert_eq!(name_group("budget").label, "B");
        assert_eq!(name_group("Été").label, "É");
        assert_eq!(name_group("_notes").label, "Other");
        assert!(name_group("9") < name_group("a") && name_group("z") < name_group("~"));

        let label = |size, dir| size_group(size, dir).label;
        assert_eq!(label(Some(1_000_000_000), false), "Huge (over 1 GB)");
        assert_eq!(label(Some(999_999_999), false), "Large (100 MB to 1 GB)");
        assert_eq!(label(Some(1_000_000), false), "Medium (1 to 100 MB)");
        assert_eq!(label(Some(999_999), false), "Small (100 KB to 1 MB)");
        assert_eq!(label(Some(1), false), "Tiny (under 100 KB)");
        assert_eq!(label(Some(0), false), "Empty");
        assert_eq!(label(None, true), "Folders");
        assert!(size_group(Some(5_000_000_000), false) < size_group(Some(0), false));
    }

    #[test]
    fn arranges_groups_with_open_folders_and_collapsed_groups() {
        // (name, depth, group): "docs" is an open folder holding two items.
        let items = vec![
            ("b.txt", 0, "Text"),
            ("docs", 0, "Folder"),
            ("docs/x.txt", 1, "Text"),
            ("docs/y.png", 1, "Image"),
            ("a.txt", 0, "Text"),
            ("pic.png", 0, "Image"),
        ];
        let arrange_with = |collapsed: &'static [&'static str]| {
            arrange(
                items.clone(),
                |item| item.1,
                |item| GroupId::new(0, 0, item.2),
                |label| collapsed.contains(&label),
            )
        };

        let (arranged, spans) = arrange_with(&[]);
        let names: Vec<&str> = arranged.iter().map(|item| item.0).collect();
        assert_eq!(
            names,
            [
                "docs",
                "docs/x.txt",
                "docs/y.png",
                "pic.png",
                "b.txt",
                "a.txt"
            ],
            "groups in order, items in their order, children with their folder"
        );
        let summary: Vec<(&str, usize, Range<usize>)> = spans
            .iter()
            .map(|s| (s.label.as_str(), s.total, s.range.clone()))
            .collect();
        assert_eq!(
            summary,
            [("Folder", 1, 0..3), ("Image", 1, 3..4), ("Text", 2, 4..6)]
        );

        let (arranged, spans) = arrange_with(&["Image"]);
        let names: Vec<&str> = arranged.iter().map(|item| item.0).collect();
        assert_eq!(
            names,
            ["docs", "docs/x.txt", "docs/y.png", "b.txt", "a.txt"]
        );
        assert!(spans[1].collapsed && spans[1].range == (3..3) && spans[1].total == 1);
        assert_eq!(spans[2].range, 3..5);
    }
}
