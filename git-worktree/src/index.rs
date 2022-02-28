use git_hash::oid;

pub mod checkout {
    use bstr::BString;
    use quick_error::quick_error;

    #[derive(Default, Clone, Copy)]
    pub struct Options {
        /// capabilities of the file system
        pub fs: crate::fs::Context,
        /// If true, we assume no file to exist in the target directory, and want exclusive access to it.
        /// This should be enabled when cloning.
        pub destination_is_initially_empty: bool,
    }

    quick_error! {
        #[derive(Debug)]
        pub enum Error {
            IllformedUtf8{ path: BString } {
                display("Could not convert path to UTF8: {}", path)
            }
            Time(err: std::time::SystemTimeError) {
                from()
                source(err)
                display("The clock was off when reading file related metadata after updating a file on disk")
            }
            Io(err: std::io::Error) {
                from()
                source(err)
                display("IO error while writing blob or reading file metadata or changing filetype")
            }
            ObjectNotFound{ oid: git_hash::ObjectId, path: std::path::PathBuf } {
                display("object {} for checkout at {} not found in object database", oid.to_hex(), path.display())
            }
        }
    }
}

pub fn checkout<Find>(
    index: &mut git_index::State,
    path: impl AsRef<std::path::Path>,
    mut find: Find,
    options: checkout::Options,
) -> Result<(), checkout::Error>
where
    Find: for<'a> FnMut(&oid, &'a mut Vec<u8>) -> Option<git_object::BlobRef<'a>>,
{
    if !options.destination_is_initially_empty {
        todo!("non-clone logic isn't implemented or vetted yet");
    }
    let root = path.as_ref();
    let mut buf = Vec::new();
    for (entry, entry_path) in index.entries_mut_with_paths() {
        // TODO: write test for that
        if entry.flags.contains(git_index::entry::Flags::SKIP_WORKTREE) {
            continue;
        }

        entry::checkout(entry, entry_path, &mut find, root, options, &mut buf)?;
    }
    Ok(())
}

pub(crate) mod entry {
    use std::{
        convert::TryInto,
        fs::{create_dir_all, OpenOptions},
        io::Write,
        time::Duration,
    };

    use bstr::BStr;
    use git_hash::oid;
    use git_index::Entry;

    use crate::index;

    pub fn checkout<Find>(
        entry: &mut Entry,
        entry_path: &BStr,
        find: &mut Find,
        root: &std::path::Path,
        index::checkout::Options {
            fs: crate::fs::Context { symlink, .. },
            ..
        }: index::checkout::Options,
        buf: &mut Vec<u8>,
    ) -> Result<(), index::checkout::Error>
    where
        Find: for<'a> FnMut(&oid, &'a mut Vec<u8>) -> Option<git_object::BlobRef<'a>>,
    {
        let dest = root.join(git_features::path::from_byte_slice(entry_path).map_err(|_| {
            index::checkout::Error::IllformedUtf8 {
                path: entry_path.to_owned(),
            }
        })?);
        create_dir_all(dest.parent().expect("entry paths are never empty"))?; // TODO: can this be avoided to create dirs when needed only?

        match entry.mode {
            git_index::entry::Mode::FILE | git_index::entry::Mode::FILE_EXECUTABLE => {
                let obj = find(&entry.id, buf).ok_or_else(|| index::checkout::Error::ObjectNotFound {
                    oid: entry.id,
                    path: root.to_path_buf(),
                })?;
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                if entry.mode == git_index::entry::Mode::FILE_EXECUTABLE {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o777);
                }

                {
                    let mut file = options.open(&dest)?;
                    file.write_all(obj.data)?;
                    // NOTE: we don't call `file.sync_all()` here knowing that some filesystems don't handle this well.
                    //       revisit this once there is a bug to fix.
                }
                update_fstat(entry, dest.symlink_metadata()?)?;
            }
            git_index::entry::Mode::SYMLINK => {
                let obj = find(&entry.id, buf).ok_or_else(|| index::checkout::Error::ObjectNotFound {
                    oid: entry.id,
                    path: root.to_path_buf(),
                })?;
                let symlink_destination = git_features::path::from_byte_slice(obj.data)
                    .map_err(|_| index::checkout::Error::IllformedUtf8 { path: obj.data.into() })?;

                if symlink {
                    symlink::symlink_auto(symlink_destination, &dest)?;
                } else {
                    std::fs::write(&dest, obj.data)?;
                }

                update_fstat(entry, std::fs::symlink_metadata(&dest)?)?;
            }
            git_index::entry::Mode::DIR => todo!(),
            git_index::entry::Mode::COMMIT => todo!(),
            _ => unreachable!(),
        }
        Ok(())
    }

    fn update_fstat(entry: &mut Entry, meta: std::fs::Metadata) -> Result<(), index::checkout::Error> {
        let ctime = meta
            .created()
            .map_or(Ok(Duration::default()), |x| x.duration_since(std::time::UNIX_EPOCH))?;
        let mtime = meta
            .modified()
            .map_or(Ok(Duration::default()), |x| x.duration_since(std::time::UNIX_EPOCH))?;

        let stat = &mut entry.stat;
        stat.mtime.secs = mtime
            .as_secs()
            .try_into()
            .expect("by 2038 we found a solution for this");
        stat.mtime.nsecs = mtime.subsec_nanos();
        stat.ctime.secs = ctime
            .as_secs()
            .try_into()
            .expect("by 2038 we found a solution for this");
        stat.ctime.nsecs = ctime.subsec_nanos();
        Ok(())
    }
}