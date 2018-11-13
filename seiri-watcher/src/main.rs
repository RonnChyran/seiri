#![feature(toowned_clone_into)]

extern crate notify;
extern crate seiri;
extern crate threadpool;
extern crate walkdir;

extern crate crossbeam;

use std::borrow::Cow;
use std::ffi::OsStr;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use crossbeam::channel::{unbounded, Receiver};
use std::thread;
use std::time::Duration;
mod utils;
mod watcher;

use seiri::config;
use seiri::config::Config;
use seiri::database;
use seiri::database::Connection;
use seiri::database::ConnectionPool;
use seiri::paths;
use seiri::Error;
use self::watcher::WatchStatus;

fn osstr_to_string(osstr: Option<&OsStr>) -> Cow<str> {
    osstr
        .and_then(|s| Some(s.to_string_lossy()))
        .unwrap_or(Cow::Borrowed(""))
}

fn process(path: &Path, config: &Config, conn: &Connection, retry: bool) {
    let track = paths::new_track_checked(path, None);
    match paths::ensure_music_folder(&config.music_folder) {
        Ok(library_path) => match track {
            Ok(track) => match paths::move_new_track(&track, &library_path.0, &library_path.1) {
                Ok(track) => {
                    database::add_track(&track, conn);
                    eprintln!("TRACKADDED::{} – {}", track.artist, track.title);
                }
                Err(_) if retry => process(path, config, conn, false),
                Err(Error::UnableToMove(_)) => {
                    eprintln!("ETRACKMOVE::{}", track.file_path.display())
                }
                Err(Error::UnableToCreateDirectory(new_directory)) => {
                    eprintln!("ECREATEDIRECTORY::{}", new_directory)
                }
                Err(_) => eprintln!("ETRACK::{}", track.file_path.display()),
            },
            Err(_) if retry => process(path, config, conn, false),
            Err(err) => match err {
                Error::UnsupportedFile(file_name) => {
                    match paths::move_non_track(&file_name, &library_path.1) {
                        Ok(()) => eprintln!("ENONTRACK::{}", osstr_to_string(file_name.file_name())),
                        Err(_) => {
                            eprintln!("ETRACKMOVE::{}", osstr_to_string(file_name.file_name()))
                        }
                    }
                }
                Error::FileIOError(file_name) => {
                    eprintln!("ETRACK::{}", osstr_to_string(file_name.file_name()))
                }
                Error::MissingRequiredTag(file_name, tag) => eprintln!(
                    "EMISSINGTAG::Track {} is missing tag {}.",
                    osstr_to_string(Path::new(&file_name).file_name()),
                    tag
                ),
                _ => eprintln!("ETRACK::Unknown Error"),
            },
        },
        Err(_) => eprintln!("ELIBRARYNOTFOUND::{}.", path.display()),
    }
}

fn wait_for_watch_root_available(folder: &str) -> (PathBuf, PathBuf) {
    println!("Waiting for folder {}...", folder);
    let wait_time = Duration::from_secs(5);
    while let Err(_) = paths::ensure_music_folder(folder) {
        thread::park_timeout(wait_time);
    }
    println!("Successfully ensured folder {}", folder);
    paths::ensure_music_folder(folder).unwrap()
}

fn begin_watch(config: Config, pool: ConnectionPool, rx: &Receiver<WatchStatus>) {
    let auto_paths = wait_for_watch_root_available(&config.music_folder);
    let watch_path = &auto_paths.1.to_str().unwrap();
    println!("Watching {}", watch_path);
    watcher::list(&watch_path, &config, &pool, process);
    // Create a channel to receive the events.
    if let Err(e) = watcher::watch(&watch_path, config, pool, process, &rx) {
        eprintln!("EWATCHER::{}", e);
    }
}

fn get_watcher_thread(rx: Receiver<WatchStatus>) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("WatchThread".to_string())
        .spawn(move || {
            let config = config::get_config();
            let pool = database::get_connection_pool();
            begin_watch(config, pool, &rx)
        })
}

fn start_watcher_watchdog(wait_time: Duration) {
    thread::spawn(move || {
        let (tx, rx) = unbounded();
        let mut tx = tx;
        let config = config::get_config();
        wait_for_watch_root_available(&config.music_folder);
        let mut _watch_thread = get_watcher_thread(rx).unwrap();
        loop {
            thread::park_timeout(wait_time);
            if tx.send(WatchStatus::KeepAlive).is_err() {
                eprintln!("EWATCHERDIED::Keep-alive failed. Watcher thread probably panicked. Restarting Watcher Thread...");
                let (new_tx, rx) = unbounded();
                tx = new_tx.clone();
                _watch_thread = get_watcher_thread(rx).unwrap();
            }

            let music_folder = paths::ensure_music_folder(&config.music_folder);
            if music_folder.is_err() {
                eprintln!("EWATCHERNOACCESS::{}", &config.music_folder);
                wait_for_watch_root_available(&config.music_folder);
                let (new_tx, rx) = unbounded();
                tx.send(WatchStatus::Exit).unwrap();
                eprintln!(
                    "EWATCHERRESTART::Requested watcher thread exit. Restarting Watcher Thread..."
                );
                tx = new_tx.clone();
                _watch_thread = get_watcher_thread(rx).unwrap();
            }
        }
    });
}

fn ensure_port(port: u16) -> Result<TcpListener, io::Error> {
    match TcpListener::bind(("localhost", port)) {
        Ok(socket) => Ok(socket),
        Err(err) => Err(err),
    }
}

fn main() {
    let _lock = ensure_port(9235).expect("Unable to acquire lock");

    let wait_time = Duration::from_secs(5);
    start_watcher_watchdog(wait_time);

    let conn = database::get_database_connection();
    utils::wait_for_exit(&conn);
}
