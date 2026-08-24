//! Single-instance GUI startup and local file-open forwarding.
//!
//! A loopback TCP listener is used instead of OS-specific named pipes or Unix
//! sockets. The endpoint is protected by an exclusive lock file, and a random
//! token in the endpoint file prevents unrelated local processes from sending
//! commands to the GUI.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use logotomy::core::settings::Settings;

const LOCK_FILE: &str = "gui-instance.lock";
const ENDPOINT_FILE: &str = "gui-instance.json";
const RETRY_FOR: Duration = Duration::from_secs(3);

#[derive(Debug, Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    token: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenRequest {
    token: String,
    paths: Vec<PathBuf>,
}

pub(crate) enum Startup {
    /// This process owns the GUI instance.
    Primary(SingleInstance),
    /// The request was delivered to an existing GUI instance.
    Forwarded,
}

pub(crate) struct SingleInstance {
    lock_file: File,
    endpoint_path: PathBuf,
    listener_addr: std::net::SocketAddr,
    stopping: Arc<AtomicBool>,
    listener_thread: Option<JoinHandle<()>>,
    initial_paths: Vec<PathBuf>,
    requests: Receiver<Vec<PathBuf>>,
}

impl SingleInstance {
    pub(crate) fn acquire(paths: Vec<PathBuf>) -> Result<Startup, String> {
        Self::acquire_in(&Settings::home_dir(), paths)
    }

    fn acquire_in(data_dir: &Path, paths: Vec<PathBuf>) -> Result<Startup, String> {
        std::fs::create_dir_all(data_dir).map_err(|e| {
            format!(
                "failed to create GUI state directory {}: {e}",
                data_dir.display()
            )
        })?;

        let paths = paths
            .into_iter()
            .map(normalize_path_for_app)
            .collect::<Vec<_>>();
        let lock_path = data_dir.join(LOCK_FILE);
        let endpoint_path = data_dir.join(ENDPOINT_FILE);
        let deadline = Instant::now() + RETRY_FOR;

        loop {
            let lock_file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|e| format!("failed to open {}: {e}", lock_path.display()))?;

            match lock_file.try_lock_exclusive() {
                Ok(()) => return Self::start_primary(lock_file, endpoint_path, paths),
                Err(error) if is_lock_contention(&error) => {
                    if try_forward(&endpoint_path, &paths) {
                        return Ok(Startup::Forwarded);
                    }
                    if Instant::now() >= deadline {
                        return Err(
                            "another logotomy GUI instance is running, but it did not accept the file-open request"
                                .to_string(),
                        );
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    return Err(format!("failed to lock {}: {error}", lock_path.display()))
                }
            }
        }
    }

    fn start_primary(
        lock_file: File,
        endpoint_path: PathBuf,
        initial_paths: Vec<PathBuf>,
    ) -> Result<Startup, String> {
        // The lock proves that no other primary is using this endpoint. This
        // also removes an endpoint left behind by a crash.
        let _ = std::fs::remove_file(&endpoint_path);
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|e| format!("failed to create GUI command listener: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("failed to configure GUI command listener: {e}"))?;
        let listener_addr = listener
            .local_addr()
            .map_err(|e| format!("failed to read GUI command listener address: {e}"))?;
        let token = format!("{:032x}", rand::random::<u128>());
        let endpoint = Endpoint {
            port: listener_addr.port(),
            token: token.clone(),
        };

        let temporary_endpoint =
            endpoint_path.with_extension(format!("tmp-{}", std::process::id()));
        let endpoint_text = serde_json::to_vec(&endpoint)
            .map_err(|e| format!("failed to encode GUI command endpoint: {e}"))?;
        if let Err(error) = write_endpoint(&temporary_endpoint, &endpoint_path, &endpoint_text) {
            let _ = lock_file.unlock();
            return Err(format!("failed to publish GUI command endpoint: {error}"));
        }

        let (tx, requests) = crossbeam_channel::unbounded();
        let stopping = Arc::new(AtomicBool::new(false));
        let stop_listener = Arc::clone(&stopping);
        let listener_thread = match thread::Builder::new()
            .name("logotomy-open-file".to_string())
            .spawn(move || listen_for_requests(listener, token, tx, stop_listener))
        {
            Ok(thread) => thread,
            Err(error) => {
                let _ = std::fs::remove_file(&endpoint_path);
                let _ = lock_file.unlock();
                return Err(format!("failed to start GUI command listener: {error}"));
            }
        };

        Ok(Startup::Primary(Self {
            lock_file,
            endpoint_path,
            listener_addr,
            stopping,
            listener_thread: Some(listener_thread),
            initial_paths,
            requests,
        }))
    }

    pub(crate) fn initial_paths(&self) -> &[PathBuf] {
        &self.initial_paths
    }

    pub(crate) fn requests(&self) -> Receiver<Vec<PathBuf>> {
        self.requests.clone()
    }
}

fn is_lock_contention(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }

    // fs2 delegates to LockFileEx on Windows. Its ERROR_LOCK_VIOLATION
    // (33) is returned as a platform-specific error instead of WouldBlock.
    // Treat sharing violations similarly because opening a lock file can
    // report that code while another process is acquiring the lock.
    #[cfg(windows)]
    return matches!(error.raw_os_error(), Some(32 | 33));

    #[cfg(not(windows))]
    false
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        // Wake the non-blocking listener so eframe shutdown does not have to
        // wait for its polling interval.
        let _ = TcpStream::connect(self.listener_addr);
        if let Some(thread) = self.listener_thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.endpoint_path);
        let _ = self.lock_file.unlock();
    }
}

fn write_endpoint(temporary: &Path, endpoint: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(temporary, bytes)?;
    if let Err(error) = std::fs::rename(temporary, endpoint) {
        let _ = std::fs::remove_file(temporary);
        return Err(error);
    }
    Ok(())
}

fn listen_for_requests(
    listener: TcpListener,
    token: String,
    requests: Sender<Vec<PathBuf>>,
    stopping: Arc<AtomicBool>,
) {
    while !stopping.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => handle_request(stream, &token, &requests),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
}

fn handle_request(mut stream: TcpStream, token: &str, requests: &Sender<Vec<PathBuf>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut bytes = Vec::new();
    if (&mut stream)
        .take(1024 * 1024)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return;
    }
    let Ok(request) = serde_json::from_slice::<OpenRequest>(&bytes) else {
        return;
    };
    if request.token != token {
        return;
    }
    let paths = request
        .paths
        .into_iter()
        .map(normalize_path_for_app)
        .collect();
    if requests.send(paths).is_ok() {
        let _ = stream.write_all(b"ok");
    }
}

fn try_forward(endpoint_path: &Path, paths: &[PathBuf]) -> bool {
    let Ok(bytes) = std::fs::read(endpoint_path) else {
        return false;
    };
    let Ok(endpoint) = serde_json::from_slice::<Endpoint>(&bytes) else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(
        &std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, endpoint.port)),
        Duration::from_millis(250),
    ) else {
        return false;
    };
    let request = OpenRequest {
        token: endpoint.token,
        paths: paths.to_vec(),
    };
    let Ok(bytes) = serde_json::to_vec(&request) else {
        return false;
    };
    if stream.write_all(&bytes).is_err() || stream.shutdown(Shutdown::Write).is_err() {
        return false;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut response = [0_u8; 2];
    stream.read_exact(&mut response).is_ok() && &response == b"ok"
}

pub(crate) fn normalize_path_for_app(path: PathBuf) -> PathBuf {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    std::fs::canonicalize(&absolute).unwrap_or(absolute)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_dir() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "logotomy_instance_test_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn second_gui_launch_forwards_paths_to_primary() {
        let dir = test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("nested").join("example.log");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "line\n").unwrap();

        let primary = match SingleInstance::acquire_in(&dir, Vec::new()).unwrap() {
            Startup::Primary(instance) => instance,
            Startup::Forwarded => panic!("test primary unexpectedly forwarded"),
        };
        let forwarded = SingleInstance::acquire_in(&dir, vec![file.clone()]).unwrap();
        assert!(matches!(forwarded, Startup::Forwarded));
        let received = primary
            .requests()
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(received, vec![std::fs::canonicalize(file).unwrap()]);

        drop(primary);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn relative_paths_are_made_absolute_before_forwarding() {
        let path = normalize_path_for_app(PathBuf::from("some-file.log"));
        assert!(path.is_absolute());
    }

    #[test]
    fn lock_contention_is_retryable() {
        assert!(is_lock_contention(&io::Error::from(io::ErrorKind::WouldBlock)));

        #[cfg(windows)]
        assert!(is_lock_contention(&io::Error::from_raw_os_error(33)));
    }
}
