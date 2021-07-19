use crate::store::{file, packed};
use bstr::{BString, ByteSlice};
use git_features::fs::walkdir::DirEntryIter;
use os_str_bytes::OsStrBytes;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// An iterator over all valid loose reference paths as seen from a particular base directory.
pub(in crate::store::file) struct SortedLoosePaths {
    base: PathBuf,
    file_walk: DirEntryIter,
    mode: LoosePathsMode,
}

enum LoosePathsMode {
    Paths,
    PathsAndNames,
}

impl SortedLoosePaths {
    pub fn at_root(path: impl AsRef<Path>, base: impl Into<PathBuf>) -> Self {
        Self::new(path.as_ref(), base.into(), LoosePathsMode::Paths)
    }

    pub fn at_root_with_names(path: impl AsRef<Path>, base: impl Into<PathBuf>) -> Self {
        Self::new(path.as_ref(), base.into(), LoosePathsMode::PathsAndNames)
    }

    fn new(path: &Path, base: PathBuf, mode: LoosePathsMode) -> Self {
        let file_walk = git_features::fs::walkdir_sorted_new(path).into_iter();
        SortedLoosePaths { base, file_walk, mode }
    }
}

impl Iterator for SortedLoosePaths {
    type Item = std::io::Result<(PathBuf, Option<BString>)>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(entry) = self.file_walk.next() {
            match entry {
                Ok(entry) => {
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let full_path = entry.path().to_owned();
                    let full_name = full_path
                        .strip_prefix(&self.base)
                        .expect("prefix-stripping cannot fail as prefix is our root")
                        .to_raw_bytes();
                    #[cfg(windows)]
                    let full_name: Vec<u8> = full_name.into_owned().replace(b"\\", b"/");

                    use LoosePathsMode::*;
                    if git_validate::reference::name_partial(full_name.as_bstr()).is_ok() {
                        let name = match self.mode {
                            Paths => None,
                            #[cfg(not(windows))]
                            PathsAndNames => Some(full_name.into_owned().into()),
                            #[cfg(windows)]
                            PathsAndNames => Some(full_name.into()),
                        };
                        return Some(Ok((full_path, name)));
                    } else {
                        continue;
                    }
                }
                Err(err) => return Some(Err(err.into_io_error().expect("no symlink related errors"))),
            }
        }
        None
    }
}

/// An iterator over all loose references as seen from a particular base directory.
pub struct Loose<'s, 'p> {
    parent: &'s file::Store,
    packed: Option<&'p packed::Buffer>,
    ref_paths: SortedLoosePaths,
    buf: Vec<u8>,
}

impl<'s, 'p> Loose<'s, 'p> {
    /// Initialize a loose reference iterator owned by `store` at the given iteration `root`, where `base` is the
    /// path to which resulting reference names should be relative to.
    pub fn at_root(
        store: &'s file::Store,
        packed: Option<&'p packed::Buffer>,
        root: impl AsRef<Path>,
        base: impl Into<PathBuf>,
    ) -> Self {
        Loose {
            parent: store,
            packed,
            ref_paths: SortedLoosePaths::at_root(root, base),
            buf: Vec::new(),
        }
    }
}

impl<'s, 'p> Iterator for Loose<'s, 'p> {
    type Item = Result<file::Reference<'s, 'p>, loose::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.ref_paths.next().map(|res| {
            res.map_err(loose::Error::Traversal)
                .and_then(|(validated_path, _name)| {
                    std::fs::File::open(&validated_path)
                        .and_then(|mut f| {
                            self.buf.clear();
                            f.read_to_end(&mut self.buf)
                        })
                        .map_err(loose::Error::ReadFileContents)
                        .and_then(|_| {
                            let relative_path = validated_path
                                .strip_prefix(&self.ref_paths.base)
                                .expect("root contains path");
                            file::Reference::try_from_path(self.parent, relative_path, &self.buf).map_err(|err| {
                                loose::Error::ReferenceCreation {
                                    err,
                                    relative_path: relative_path.into(),
                                }
                            })
                        })
                })
        })
    }
}

impl file::Store {
    /// Return an iterator over all loose references, notably not including any packed ones, in file system order.
    /// Each of the references may fail to parse and the iterator will not stop if parsing fails, allowing the caller
    /// to see all files that look like references whether valid or not.
    ///
    /// Reference files that do not constitute valid names will be silently ignored.
    ///
    /// See [`Store::packed()`][file::Store::packed()] for interacting with packed references.
    pub fn loose_iter(&self) -> std::io::Result<Loose<'_>> {
        let refs = self.refs_dir();
        if !refs.is_dir() {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        Ok(Loose::at_root(self, refs, self.base.clone()))
    }

    /// Return an iterator over all loose references that start with the given `prefix`.
    ///
    /// Otherwise it's similar to [`loose_iter()`][file::Store::loose_iter()].
    pub fn loose_iter_prefixed(&self, prefix: impl AsRef<Path>) -> std::io::Result<Loose<'_>> {
        let prefix = self.validate_prefix(prefix.as_ref())?;
        Ok(Loose::at_root(self, self.base.join(prefix), self.base.clone()))
    }

    pub(in crate::store::file) fn refs_dir(&self) -> PathBuf {
        self.base.join("refs")
    }
    pub(in crate::store::file) fn validate_prefix<'a>(&self, prefix: &'a Path) -> std::io::Result<&'a Path> {
        if prefix.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "prefix must be a relative path, like 'refs/heads'",
            ));
        }
        for component in prefix.components() {
            use std::path::Component::*;
            if matches!(component, CurDir | ParentDir) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Refusing to handle prefixes with relative path components",
                ));
            }
        }
        Ok(prefix)
    }
}

///
pub mod loose {
    mod error {
        use crate::file;
        use quick_error::quick_error;
        use std::{io, path::PathBuf};

        quick_error! {
            /// The error returned by [file::iter::Loose] iteration.
            #[derive(Debug)]
            #[allow(missing_docs)]
            pub enum Error {
                Traversal(err: io::Error) {
                    display("The file system could not be traversed")
                    source(err)
                }
                ReadFileContents(err: io::Error) {
                    display("The ref file could not be read in full")
                    source(err)
                }
                ReferenceCreation{ err: file::reference::decode::Error, relative_path: PathBuf } {
                    display("The reference at '{}' could not be instantiated", relative_path.display())
                    source(err)
                }
            }
        }
    }
    pub use error::Error;
}