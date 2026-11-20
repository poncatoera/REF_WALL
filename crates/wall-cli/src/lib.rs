//! The import side of `wall-cli`, also used by the desktop app: files into
//! a library ([`media`]) and the old Electron Wall's library ([`old_wall`]).

pub mod media;
pub mod old_wall;

pub type Res<T> = Result<T, String>;
