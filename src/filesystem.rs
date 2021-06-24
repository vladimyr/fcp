//! A collection of utilities augmenting the standard library's filesystem capabilities, extending
//! them to cover the full gamut of POSIX file types, and wrapping them in order to improve the
//! usefulness of error messages by providing additional context.

use lazy_static::lazy_static;
use nix::sys::stat::Mode;
use nix::unistd;
use std::convert::TryInto;
use std::error::Error as BaseError;
use std::fmt;
use std::fs::{self, DirBuilder, File, Metadata, OpenOptions, Permissions, ReadDir};
use std::os::unix::fs::{self as unix, DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::path::{self, Component, Path, PathBuf};

#[derive(Debug)]
pub struct Error {
    message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl<T: BaseError> From<T> for Error {
    fn from(other: T) -> Self {
        Error {
            message: other.to_string(),
        }
    }
}

impl Error {
    pub fn new(message: String) -> Self {
        Error { message }
    }
}

macro_rules! wrap {
    ($namespace:ident, $function:ident, $payload:ty) => {
        pub fn $function<P: AsRef<Path>>(path: P) -> Result<$payload, Error> {
            $namespace::$function(path.as_ref())
                .map_err(|err| Error::new(format!("{}: {}", path.as_ref().display(), err)))
        }
    };
}

macro_rules! wrap2 {
    ($function:ident, $namespace:ident, $payload:ty) => {
        pub fn $function<P: AsRef<Path>, Q: AsRef<Path>>(
            source: P,
            dest: Q,
        ) -> Result<$payload, Error> {
            let (source, dest) = (source.as_ref(), dest.as_ref());
            $namespace::$function(source, dest).map_err(|err| {
                Error::new(format!("{}, {}: {}", source.display(), dest.display(), err))
            })
        }
    };
}

wrap!(fs, symlink_metadata, Metadata);
wrap!(fs, read_link, PathBuf);
wrap!(fs, read_dir, ReadDir);
wrap!(fs, remove_dir_all, ());
wrap!(fs, remove_file, ());
wrap!(fs, canonicalize, PathBuf);
wrap!(File, open, File);
wrap2!(symlink, unix, ());
wrap2!(copy, fs, u64);

macro_rules! make_error_message {
    ($path:ident) => {
        |err| Error::new(format!("{}: {}", $path.display(), err));
    };
}

pub fn create_dir<P: AsRef<Path>>(path: P, mode: u32) -> Result<(), Error> {
    let path = path.as_ref();
    DirBuilder::new()
        .mode(mode)
        .create(path)
        .map_err(make_error_message!(path))
}

pub fn create<P: AsRef<Path>>(path: P, mode: u32) -> Result<File, Error> {
    let path = path.as_ref();
    OpenOptions::new()
        .mode(mode)
        .truncate(true)
        .write(true)
        .create(true)
        .open(path)
        .map_err(make_error_message!(path))
}

pub fn mkfifo<P: AsRef<Path>>(path: P, permissions: Permissions) -> Result<(), Error> {
    let path = path.as_ref();
    let mode = Mode::from_bits_truncate(permissions.mode().try_into()?);
    unistd::mkfifo(path, mode).map_err(make_error_message!(path))
}

#[derive(Debug)]
pub enum FileType {
    Regular,
    Directory(Metadata),
    Symlink,
    Fifo(Metadata),
    Socket,
    CharacterDevice(Metadata),
    BlockDevice(Metadata),
}

pub fn file_type(path: &Path) -> Result<FileType, Error> {
    let metadata = symlink_metadata(path)?;
    let file_type = metadata.file_type();
    Ok(if file_type.is_file() {
        FileType::Regular
    } else if file_type.is_dir() {
        FileType::Directory(metadata)
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else if file_type.is_fifo() {
        FileType::Fifo(metadata)
    } else if file_type.is_socket() {
        FileType::Socket
    } else if file_type.is_char_device() {
        FileType::CharacterDevice(metadata)
    } else if file_type.is_block_device() {
        FileType::BlockDevice(metadata)
    } else {
        unreachable!(
            "{}: file appears to exist but is an unknown type",
            path.display()
        );
    })
}

lazy_static! {
    static ref ROOT: &'static Path = Path::new("/");
}

pub fn semicanonicalize(path: &Path, current_dir: &Path) -> Result<PathBuf, Error> {
    fn unknown_error(path: path::Display) -> Error {
        Error::new(format!(
            "{}: an unknown error occurred while processing this path",
            path
        ))
    }

    let mut components = path.components();
    let last = components
        .next_back()
        .ok_or_else(|| Error::new("Empty file path provided as an argument".to_string()))?;
    let mut prefix = components.as_path().to_path_buf();
    let prefix_exists = !prefix.as_os_str().is_empty();
    if prefix_exists {
        prefix = canonicalize(prefix)?;
    }
    Ok(match last {
        Component::CurDir if prefix_exists => prefix,
        Component::CurDir => current_dir.to_path_buf(),
        Component::Normal(filename) if prefix_exists => prefix.join(filename),
        Component::Normal(filename) => current_dir.join(filename),
        Component::ParentDir if prefix_exists => {
            prefix.pop();
            prefix
        }
        Component::ParentDir if current_dir == *ROOT => current_dir.to_path_buf(),
        Component::ParentDir => current_dir
            .parent()
            .ok_or_else(|| unknown_error(path.display()))?
            .to_path_buf(),
        Component::RootDir if !prefix_exists => ROOT.to_path_buf(),
        Component::RootDir => return Err(unknown_error(path.display())),
        // This is unreachable as opposed to a normal error because fcp shouldn't even compile on
        // non-unix systems due to depending on the nix crate, so this really should never run.
        Component::Prefix(_) => unreachable!("fcp does not support non-unix systems"),
    })
}
