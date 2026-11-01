//! Command-line flags: `markdownmate --new` / `--today` (the desktop file's actions) next to
//! the files and folders `markdownmate` opens. GLib parses the options too (for `--help`);
//! this decides what a command line means and rejects the combinations that make no sense.

/// A quick-capture command from the dock menu or a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Quick {
    /// New Note in the active window's open folder.
    New,
    /// Today's Note in the active window's open folder, else the last session's folder.
    Today,
}

impl Quick {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Quick::New => "new",
            Quick::Today => "today",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Quick> {
        match name {
            "new" => Some(Quick::New),
            "today" => Some(Quick::Today),
            _ => None,
        }
    }
}

/// What the command line asks for (arguments after the program name).
pub(crate) fn parse<S: AsRef<str>>(args: &[S]) -> Result<Option<Quick>, String> {
    let mut quick = None;
    let mut paths = 0;
    let mut only_paths = false;
    for arg in args {
        let arg = arg.as_ref();
        let flag = match arg {
            _ if only_paths => None,
            "--" => {
                only_paths = true;
                continue;
            }
            "--new" => Some(Quick::New),
            "--today" => Some(Quick::Today),
            // Other options (`--help`, GTK's) are GLib's to handle.
            _ if arg.starts_with('-') => continue,
            _ => None,
        };
        match flag {
            Some(flag) => {
                if quick.is_some_and(|q| q != flag) {
                    return Err("--new and --today cannot be used together".into());
                }
                quick = Some(flag);
            }
            None => paths += 1,
        }
    }
    if quick.is_some() && paths > 0 {
        return Err(format!(
            "--{} takes no file or folder",
            quick.map_or("", Quick::name)
        ));
    }
    Ok(quick)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags() {
        assert_eq!(parse(&["--new"]), Ok(Some(Quick::New)));
        assert_eq!(parse(&["--today"]), Ok(Some(Quick::Today)));
        assert_eq!(parse(&["--today", "--today"]), Ok(Some(Quick::Today)));
        assert_eq!(parse::<&str>(&[]), Ok(None));
    }

    #[test]
    fn paths_open_as_before() {
        assert_eq!(parse(&["note.md", "/home/a/Notes"]), Ok(None));
        assert_eq!(parse(&["--", "--new"]), Ok(None));
        assert_eq!(parse(&["--gapplication-service"]), Ok(None));
    }

    #[test]
    fn both_flags_or_a_flag_with_a_path_is_an_error() {
        assert!(parse(&["--new", "--today"]).is_err());
        assert!(parse(&["--today", "note.md"]).is_err());
        assert!(parse(&["note.md", "--new"]).is_err());
    }

    #[test]
    fn names_round_trip() {
        for q in [Quick::New, Quick::Today] {
            assert_eq!(Quick::from_name(q.name()), Some(q));
        }
        assert_eq!(Quick::from_name("x"), None);
    }
}
