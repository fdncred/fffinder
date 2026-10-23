//! `--pick` mode: print the chosen paths as JSON and exit, so scripts (and
//! `fff pick` in Nushell) can use the window as a file chooser.

use std::io::Write as _;
use std::path::PathBuf;

/// The picker's whole stdout: one JSON array of paths and a newline.
pub fn render(paths: &[PathBuf]) -> String {
    let list: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut out = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into());
    out.push('\n');
    out
}

/// Print the chosen paths and exit successfully.
pub fn choose(paths: &[PathBuf]) -> ! {
    // When stdout is the terminal the window was drawn in, the paths belong
    // on the main screen, not the alternate one about to be cleared.
    crate::term::restore();
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(render(paths).as_bytes());
    let _ = out.flush();
    std::process::exit(0)
}

/// Exit without a choice.
pub fn cancel() -> ! {
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_one_json_array() {
        let out = render(&["/a b/c.txt".into(), "/d\"e".into()]);
        assert_eq!(out, "[\"/a b/c.txt\",\"/d\\\"e\"]\n");
        let parsed: Vec<String> = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(parsed.len(), 2);
    }
}
