use std::fs;
use std::path::PathBuf;

use git_config::{File, Source};
use tempfile::tempdir;

use crate::file::cow_str;

/// Escapes backslash when writing a path as string so that it is a valid windows path
pub(crate) fn escape_backslashes(path: impl AsRef<std::path::Path>) -> String {
    path.as_ref().to_str().unwrap().replace('\\', "\\\\")
}

mod from_path_no_includes {
    #[test]
    fn file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config");

        let err = git_config::File::from_path_no_includes(config_path, git_config::Source::Local).unwrap_err();
        assert!(
            matches!(err,  git_config::file::from_paths::Error::Io(io_error) if io_error.kind() == std::io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn single_path() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config");
        std::fs::write(config_path.as_path(), b"[core]\nboolean = true").unwrap();

        let config = git_config::File::from_path_no_includes(config_path, git_config::Source::Local).unwrap();

        assert_eq!(config.raw_value("core", None, "boolean").unwrap().as_ref(), "true");
        assert_eq!(config.num_values(), 1);
    }
}

#[test]
fn multiple_paths_single_value() -> crate::Result {
    let dir = tempdir()?;

    let a_path = dir.path().join("a");
    fs::write(a_path.as_path(), b"[core]\na = true")?;

    let b_path = dir.path().join("b");
    fs::write(b_path.as_path(), b"[core]\nb = true")?;

    let c_path = dir.path().join("c");
    fs::write(c_path.as_path(), b"[core]\nc = true")?;

    let d_path = dir.path().join("d");
    fs::write(d_path.as_path(), b"[core]\na = false")?;

    let paths = vec![a_path, b_path, c_path, d_path];
    let config = File::from_paths_metadata(into_meta(paths), Default::default())?;

    assert_eq!(config.boolean("core", None, "a"), Some(Ok(false)));
    assert_eq!(config.boolean("core", None, "b"), Some(Ok(true)));
    assert_eq!(config.boolean("core", None, "c"), Some(Ok(true)));
    assert_eq!(config.num_values(), 4);
    assert_eq!(config.sections().count(), 4, "each value is in a dedicated section");

    Ok(())
}

#[test]
fn frontmatter_is_maintained_in_multiple_files() -> crate::Result {
    let dir = tempdir()?;

    let a_path = dir.path().join("a");
    fs::write(a_path.as_path(), b";before a\n[core]\na = true")?;

    let b_path = dir.path().join("b");
    fs::write(b_path.as_path(), b";before b\n [core]\nb = true")?;

    let c_path = dir.path().join("c");
    fs::write(c_path.as_path(), b"# nothing in c")?;

    let d_path = dir.path().join("d");
    fs::write(d_path.as_path(), b"\n; nothing in d")?;

    let paths = vec![a_path, b_path, c_path, d_path];
    let mut config = File::from_paths_metadata(into_meta(paths), Default::default())?;

    assert_eq!(
        config.to_string(),
        ";before a\n[core]\na = true\n;before b\n [core]\nb = true\n# nothing in c\n; nothing in d"
    );

    config.append(config.clone());
    assert_eq!(
        config.to_string(),
        ";before a\n[core]\na = true\n;before b\n [core]\nb = true\n# nothing in c\n; nothing in d\n;before a\n[core]\na = true\n;before b\n [core]\nb = true\n# nothing in c\n; nothing in d",
        "other files post-section matter works as well, adding newlines as needed"
    );
    Ok(())
}

#[test]
fn multiple_paths_multi_value() -> crate::Result {
    let dir = tempdir()?;

    let a_path = dir.path().join("a");
    fs::write(a_path.as_path(), b"[core]\nkey = a")?;

    let b_path = dir.path().join("b");
    fs::write(b_path.as_path(), b"[core]\nkey = b")?;

    let c_path = dir.path().join("c");
    fs::write(c_path.as_path(), b"[core]\nkey = c")?;

    let d_path = dir.path().join("d");
    fs::write(d_path.as_path(), b"[include]\npath = d_path")?;

    let e_path = dir.path().join("e");
    fs::write(e_path.as_path(), b"[include]\npath = e_path")?;

    let paths_and_source = vec![
        (a_path, Source::System),
        (b_path, Source::Global),
        (c_path, Source::User),
        (d_path, Source::Worktree),
        (e_path, Source::Local),
    ];

    let config = File::from_paths_metadata(
        paths_and_source
            .iter()
            .map(|(p, s)| git_config::file::Metadata::try_from_path(p, *s).unwrap()),
        Default::default(),
    )?;

    assert_eq!(
        config.strings("core", None, "key"),
        Some(vec![cow_str("a"), cow_str("b"), cow_str("c"),])
    );

    assert_eq!(
        config.strings("include", None, "path"),
        Some(vec![cow_str("d_path"), cow_str("e_path")])
    );

    assert_eq!(config.num_values(), 5);
    assert_eq!(
        config
            .sections()
            .map(|s| (
                s.meta().path.as_ref().expect("each section has file source").to_owned(),
                s.meta().source,
                s.meta().level
            ))
            .collect::<Vec<_>>(),
        paths_and_source.into_iter().map(|(p, s)| (p, s, 0)).collect::<Vec<_>>(),
        "sections are added in order and their path and sources are set as given, levels are 0 for the non-included ones"
    );
    Ok(())
}

fn into_meta(paths: impl IntoIterator<Item = PathBuf>) -> impl IntoIterator<Item = git_config::file::Metadata> {
    paths
        .into_iter()
        .map(|p| git_config::file::Metadata::try_from_path(p, git_config::Source::Local).unwrap())
}

mod includes {
    mod conditional;
    mod unconditional;
}
