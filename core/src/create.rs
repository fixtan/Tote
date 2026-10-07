//! 書庫の作成（ZIP / 7z / tar.gz / tar）。入力の洗い出しは `plan`、ここは形式ごとの書き出し。

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::plan::{self, Item};
use crate::{CompressFormat, Error, Options, Summary, default_output_stem, io_err, mtime_of, unique_path};

/// `opts.format` の書庫を作る。失敗時は作りかけの出力ファイルを削除する。
pub fn create(inputs: &[PathBuf], opts: &Options) -> Result<Summary, Error> {
    if inputs.is_empty() {
        return Err(Error::NoInput);
    }
    for p in inputs {
        if fs::symlink_metadata(p).is_err() {
            return Err(Error::NotFound(p.clone()));
        }
    }
    if opts.password.as_deref().is_some_and(str::is_empty) {
        return Err(Error::Unsupported("パスワードが空です".into()));
    }
    if opts.password.is_some() && !opts.format.supports_password() {
        return Err(Error::Unsupported(format!("{} はパスワードに対応していません", opts.format.label())));
    }

    let output = match &opts.output {
        Some(o) => o.clone(),
        None => {
            let (dir, stem) = default_output_stem(inputs)?;
            unique_path(&dir, &stem, opts.format.extension())
        }
    };

    // 途中で止まっても、中途半端な書庫が最終的な名前で残らないよう、同じ場所の一時ファイルへ書いてから差し替える
    let tmp = tmp_path(&output);
    let file = File::create(&tmp).map_err(io_err(&tmp))?;
    let skip: Vec<PathBuf> = [&tmp, &output].iter().filter_map(|p| fs::canonicalize(p).ok()).collect();

    let result = plan::collect(inputs, &skip)
        .and_then(|plan| {
            let (files, dirs) = match opts.format {
                CompressFormat::Zip => write_zip(file, &plan.items, opts, &tmp)?,
                CompressFormat::SevenZ => write_7z(file, &plan.items, opts, &tmp)?,
                CompressFormat::TarGz | CompressFormat::Tar => write_tar(file, &plan.items, opts, &tmp)?,
            };
            Ok(Summary { output: output.clone(), files, dirs, skipped: plan.skipped })
        })
        .and_then(|s| fs::rename(&tmp, &output).map(|()| s).map_err(io_err(&output)));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn tmp_path(output: &Path) -> PathBuf {
    let mut n = output.file_name().map(|s| s.to_os_string()).unwrap_or_default();
    n.push(".tote-part");
    output.with_file_name(n)
}

/// ZIP を作る（`create` の ZIP 固定版）。
pub fn create_zip(inputs: &[PathBuf], opts: &Options) -> Result<Summary, Error> {
    create(inputs, &Options { format: CompressFormat::Zip, ..opts.clone() })
}

fn count(items: &[Item]) -> (usize, usize) {
    let dirs = items.iter().filter(|i| i.is_dir()).count();
    (items.len() - dirs, dirs)
}

// ---------------------------------------------------------------- ZIP

fn write_zip(file: File, items: &[Item], opts: &Options, output: &Path) -> Result<(usize, usize), Error> {
    let mut base = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    match opts.level {
        Some(0) => base = base.compression_method(CompressionMethod::Stored),
        Some(l) => base = base.compression_level(Some(l)),
        None => {}
    }
    let mut zip = ZipWriter::new(BufWriter::new(file));
    for it in items {
        let o = base.last_modified_time(mtime_of(&it.meta));
        if it.is_dir() {
            zip.add_directory(format!("{}/", it.name), o)?;
        } else {
            let mut o = o.large_file(it.meta.len() >= 0xFFFF_FFFF);
            if let Some(pw) = &opts.password {
                o = o.with_aes_encryption(zip::AesMode::Aes256, pw);
            }
            zip.start_file(&it.name, o)?;
            let mut f = File::open(&it.path).map_err(io_err(&it.path))?;
            io::copy(&mut f, &mut zip).map_err(io_err(&it.path))?;
        }
    }
    let mut w = zip.finish()?;
    w.flush().map_err(io_err(output))?;
    Ok(count(items))
}

// ---------------------------------------------------------------- 7z

/// 最初に読んだときに開き、読み終えたら閉じるファイル（固体圧縮で何千ものファイルを同時に開かないため）
struct LazyFile {
    path: PathBuf,
    file: Option<File>,
    done: bool,
}

impl Read for LazyFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.file.is_none() {
            self.file = Some(File::open(&self.path)?);
        }
        let n = self.file.as_mut().map(|f| f.read(buf)).transpose()?.unwrap_or(0);
        if n == 0 {
            self.file = None;
            self.done = true;
        }
        Ok(n)
    }
}

fn sz_err(e: sevenz_rust2::Error) -> Error {
    Error::Archive(format!("7zの書き出しに失敗しました: {e}"))
}

fn write_7z(file: File, items: &[Item], opts: &Options, output: &Path) -> Result<(usize, usize), Error> {
    use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, EncoderConfiguration, EncoderMethod, Password, SourceReader};

    let mut w = ArchiveWriter::new(BufWriter::new(file)).map_err(sz_err)?;
    let mut methods: Vec<EncoderConfiguration> = Vec::new();
    if let Some(pw) = &opts.password {
        methods.push(AesEncoderOptions::new(Password::new(pw)).into());
    }
    match opts.level {
        Some(0) => methods.push(EncoderConfiguration::new(EncoderMethod::COPY)),
        Some(l) => methods.push(Lzma2Options::from_level(l.clamp(1, 9) as u32).into()),
        None => methods.push(Lzma2Options::from_level(5).into()),
    }
    w.set_content_methods(methods);
    // 7-Zip の既定と同じく、名前の暗号化は頼まれたときだけ
    w.set_encrypt_header(opts.password.is_some() && opts.encrypt_names);

    let mut stream_entries = Vec::new();
    let mut stream_readers = Vec::new();
    for it in items {
        let entry = ArchiveEntry::from_path(&it.path, it.name.clone());
        if it.is_dir() {
            w.push_archive_entry::<&[u8]>(entry, None).map_err(sz_err)?;
        } else if opts.solid {
            stream_entries.push(entry);
            stream_readers.push(SourceReader::new(LazyFile { path: it.path.clone(), file: None, done: false }));
        } else {
            let f = File::open(&it.path).map_err(io_err(&it.path))?;
            w.push_archive_entry(entry, Some(f)).map_err(sz_err)?;
        }
    }
    if !stream_entries.is_empty() {
        w.push_archive_entries(stream_entries, stream_readers).map_err(sz_err)?;
    }
    let mut out = w.finish().map_err(|e| io_err(output)(e))?;
    out.flush().map_err(io_err(output))?;
    Ok(count(items))
}

// ---------------------------------------------------------------- tar / tar.gz

fn write_tar(file: File, items: &[Item], opts: &Options, output: &Path) -> Result<(usize, usize), Error> {
    let sink = BufWriter::new(file);
    if opts.format == CompressFormat::TarGz {
        let level = flate2::Compression::new(opts.level.map_or(6, |l| l.clamp(1, 9) as u32));
        let gz = flate2::write::GzEncoder::new(sink, level);
        let gz = tar_items(gz, items)?;
        let mut w = gz.finish().map_err(io_err(output))?;
        w.flush().map_err(io_err(output))?;
    } else {
        let mut w = tar_items(sink, items)?;
        w.flush().map_err(io_err(output))?;
    }
    Ok(count(items))
}

fn tar_items<W: Write>(w: W, items: &[Item]) -> Result<W, Error> {
    let mut b = tar::Builder::new(w);
    for it in items {
        b.append_path_with_name(&it.path, &it.name).map_err(io_err(&it.path))?;
    }
    b.into_inner().map_err(|e| Error::Io { path: PathBuf::new(), source: e })
}
