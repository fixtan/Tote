//! ISO（未対応。形式を足すときの置き場所）。
//!
//! 実装するときは `Backend::list` で ISO9660/Joliet（日本語名）/UDF のディレクトリを辿って `Entry` を作り、
//! `walk` で `sink.file(index, reader)` に中身を渡す。画面側は変更不要（`formats/mod.rs` の手順を参照）。

use std::path::Path;

use super::{Backend, Listing, Sink};
use crate::Error;
use crate::view::Entry;

pub(crate) struct IsoBackend;

impl Backend for IsoBackend {
    fn list(&self, _path: &Path) -> Result<Listing, Error> {
        Err(Error::Unsupported("ISOはまだ対応していません（エクスプローラーでマウントして開けます）".to_string()))
    }

    fn walk(&self, _path: &Path, _entries: &[Entry], _sink: &mut dyn Sink) -> Result<(), Error> {
        Err(Error::Unsupported("ISOはまだ対応していません".to_string()))
    }
}
