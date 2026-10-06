//! 複数選択の集約。
//!
//! エクスプローラーの右クリック動詞（レジストリ方式）は、複数選択すると
//! 選択数ぶんのプロセスを（ほぼ同時に）起動する。そこで:
//!   1. 各プロセスは自分の引数を「スプールフォルダ」に1ファイルとして書く
//!   2. ロックを取れた1プロセスだけが「リーダー」になり、新しい書き込みが
//!      落ち着く（DEBOUNCE の間増えない）まで待ってから全部まとめて処理する
//!   3. ロックを取れなかったプロセスは書き込んだだけで即終了する
//! リーダーは処理後・ロック解放後にも取りこぼしを再確認するので、遅れて来た
//! プロセスの引数が失われることはない。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, SystemTime};

pub const DEBOUNCE: Duration = Duration::from_millis(300);
const POLL: Duration = Duration::from_millis(60);
const MAX_WAIT: Duration = Duration::from_secs(8);
/// これより古いロック/ジョブは異常終了の残骸とみなす
const STALE: Duration = Duration::from_secs(60);

pub fn default_spool_dir() -> PathBuf {
    std::env::temp_dir().join("tote-spool")
}

fn lock_path(dir: &Path) -> PathBuf {
    dir.join("leader.lock")
}

fn age(p: &Path) -> Option<Duration> {
    fs::metadata(p).ok()?.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok())
}

/// 自分の引数をジョブとして書く。.tmp に書いてから rename するので、
/// リーダーが書きかけを読むことはない。
pub fn submit(dir: &Path, paths: &[PathBuf]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let name = format!("{nanos:040}-{}", std::process::id());
    let tmp = dir.join(format!("{name}.tmp"));
    let job = dir.join(format!("{name}.job"));
    {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        for p in paths {
            writeln!(f, "{}", p.to_string_lossy())?;
        }
    }
    fs::rename(tmp, job)
}

fn job_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "job"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn try_lock(dir: &Path) -> Option<Lock> {
    let lp = lock_path(dir);
    if age(&lp).is_some_and(|a| a > STALE) {
        let _ = fs::remove_file(&lp);
    }
    OpenOptions::new().write(true).create_new(true).open(&lp).ok().map(|_| Lock(lp))
}

/// 新しいジョブが増えなくなるまで待って、全ジョブのパスを回収する。
fn wait_and_take(dir: &Path, debounce: Duration) -> Vec<PathBuf> {
    let started = std::time::Instant::now();
    let mut last = job_files(dir).len();
    let mut quiet_since = std::time::Instant::now();
    loop {
        sleep(POLL);
        let now = job_files(dir).len();
        if now != last {
            last = now;
            quiet_since = std::time::Instant::now();
        }
        if quiet_since.elapsed() >= debounce || started.elapsed() >= MAX_WAIT {
            break;
        }
    }

    let mut out = Vec::new();
    for j in job_files(dir) {
        if age(&j).is_some_and(|a| a > STALE) {
            let _ = fs::remove_file(&j);
            continue;
        }
        if let Ok(body) = fs::read_to_string(&j) {
            out.extend(body.lines().filter(|l| !l.is_empty()).map(PathBuf::from));
        }
        let _ = fs::remove_file(&j);
    }
    out
}

/// リーダーになれたら、溜まったジョブをバッチにして `handle` に渡す。
/// なれなかった（別のリーダーが処理中）場合は何もせず戻る。
pub fn drain_as_leader(dir: &Path, debounce: Duration, mut handle: impl FnMut(Vec<PathBuf>)) {
    loop {
        let Some(lock) = try_lock(dir) else { return };
        loop {
            let batch = wait_and_take(dir, debounce);
            if !batch.is_empty() {
                handle(batch);
            }
            if job_files(dir).is_empty() {
                break;
            }
        }
        drop(lock);
        // ロック解放の直前に書かれたジョブの取りこぼし防止
        if job_files(dir).is_empty() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn many_simultaneous_processes_become_one_batch() {
        let dir = tempfile::tempdir().unwrap();
        let batches: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::default();
        let n = 40;

        let handles: Vec<_> = (0..n)
            .map(|i| {
                let d = dir.path().to_path_buf();
                let b = Arc::clone(&batches);
                std::thread::spawn(move || {
                    // 実プロセスの起動間隔（数ms）を模す
                    sleep(Duration::from_millis(i as u64 * 3));
                    submit(&d, &[PathBuf::from(format!("file{i}.txt"))]).unwrap();
                    drain_as_leader(&d, Duration::from_millis(200), |batch| b.lock().unwrap().push(batch));
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let b = batches.lock().unwrap();
        assert_eq!(b.len(), 1, "バッチが分割された: {:?}", b.iter().map(Vec::len).collect::<Vec<_>>());
        assert_eq!(b[0].len(), n);
        assert!(job_files(dir.path()).is_empty());
        assert!(!lock_path(dir.path()).exists());
    }

    #[test]
    fn late_arrival_is_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let batches: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::default();

        submit(dir.path(), &[PathBuf::from("a")]).unwrap();
        let d = dir.path().to_path_buf();
        let late = std::thread::spawn(move || {
            // リーダーが待機を終えて処理中のタイミングで到着
            sleep(Duration::from_millis(450));
            submit(&d, &[PathBuf::from("b")]).unwrap();
        });
        let b2 = Arc::clone(&batches);
        drain_as_leader(dir.path(), Duration::from_millis(150), |batch| {
            sleep(Duration::from_millis(400)); // 処理に時間がかかる
            b2.lock().unwrap().push(batch);
        });
        late.join().unwrap();

        let total: usize = batches.lock().unwrap().iter().map(Vec::len).sum();
        assert_eq!(total, 2);
    }
}
