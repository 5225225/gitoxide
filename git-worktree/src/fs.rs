use std::path::Path;

/// Common knowledge about the worktree that is needed across most interactions with the work tree
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone, Copy)]
pub struct Context {
    /// If true, the filesystem will store paths as decomposed unicode, i.e. `ä` becomes `"a\u{308}"`, which means that
    /// we have to turn these forms back from decomposed to precomposed unicode before storing it in the index or generally
    /// using it. This also applies to input received from the command-line, so callers may have to be aware of this and
    /// perform conversions accordingly.
    /// If false, no conversions will be performed.
    pub precompose_unicode: bool,
    /// If true, the filesystem ignores the case of input, which makes `A` the same file as `a`.
    /// This is also called case-folding.
    pub ignore_case: bool,
    /// If true, we assume the the executable bit is honored as part of the files mode. If false, we assume the file system
    /// ignores the executable bit, hence it will be reported as 'off' even though we just tried to set it to be on.
    pub file_mode: bool,
    /// If true, the file system supports symbolic links and we should try to create them. Otherwise symbolic links will be checked
    /// out as files which contain the link as text.
    pub symlink: bool,
}

impl Context {
    /// try to determine all values in this context by probing them in the given `git_dir`, which
    /// should be on the file system the git repository is located on.
    /// `git_dir` is a typical git repository, expected to be populated with the typical files like `config`.
    ///
    /// All errors are ignored and interpreted on top of the default for the platform the binary is compiled for.
    pub fn probe(git_dir: impl AsRef<std::path::Path>) -> Self {
        let root = git_dir.as_ref();
        let ctx = Context::default();
        Context {
            symlink: Self::probe_symlink(root).unwrap_or(ctx.symlink),
            ignore_case: Self::probe_ignore_case(root).unwrap_or(ctx.ignore_case),
            precompose_unicode: Self::probe_precompose_unicode(root).unwrap_or(ctx.precompose_unicode),
            ..ctx
        }
    }

    fn probe_ignore_case(git_dir: &Path) -> std::io::Result<bool> {
        std::fs::metadata(git_dir.join("cOnFiG")).map(|_| true).or_else(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(err)
            }
        })
    }

    fn probe_precompose_unicode(root: &Path) -> std::io::Result<bool> {
        let precomposed = "ä";
        let decomposed = "a\u{308}";

        let precomposed = root.join(precomposed);
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&precomposed)?;
        let res = root.join(decomposed).symlink_metadata().map(|_| true);
        std::fs::remove_file(precomposed)?;
        res
    }

    fn probe_symlink(root: &Path) -> std::io::Result<bool> {
        let src_path = root.join("__link_src_file");
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&src_path)?;
        let link_path = root.join("__file_link");
        if symlink::symlink_file(&src_path, &link_path).is_err() {
            std::fs::remove_file(&src_path)?;
            return Ok(false);
        }

        let res = std::fs::symlink_metadata(&link_path).map(|m| m.is_symlink());
        let cleanup = std::fs::remove_file(&src_path);
        symlink::remove_symlink_file(&link_path)
            .or_else(|_| std::fs::remove_file(&link_path))
            .and(cleanup)?;
        res
    }
}

#[cfg(windows)]
impl Default for Context {
    fn default() -> Self {
        Context {
            precompose_unicode: false,
            ignore_case: true,
            file_mode: false,
            symlink: false,
        }
    }
}

#[cfg(target_os = "macos")]
impl Default for Context {
    fn default() -> Self {
        Context {
            precompose_unicode: true,
            ignore_case: true,
            file_mode: true,
            symlink: true,
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
impl Default for Context {
    fn default() -> Self {
        Context {
            precompose_unicode: false,
            ignore_case: false,
            file_mode: true,
            symlink: true,
        }
    }
}