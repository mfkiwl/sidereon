use std::error::Error;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

const PTY_ROWS: u16 = 30;
const PTY_COLS: u16 = 120;
const SCREEN_TIMEOUT: Duration = Duration::from_secs(20);
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[test]
fn replay_tui_runs_in_real_pty_and_restores_terminal() -> TestResult {
    let obs = fixture(&["obs", "ESBC00DNK_R_20201770000_01D_30S_MO_trim.rnx"]);
    let nav = fixture(&["nav", "ESBC00DNK_R_20201770000_01D_MN.rnx"]);
    let mut session = spawn_pty(&[
        "tui",
        "--obs",
        obs.to_str().expect("utf-8 fixture path"),
        "--nav",
        nav.to_str().expect("utf-8 fixture path"),
        "--paused",
    ])?;

    let screen = session.wait_for_screen(SCREEN_TIMEOUT, |screen| {
        screen.contains("Solution") && screen.contains("CEP") && screen.contains("epoch 1/2")
    })?;
    assert!(
        screen.contains("solved"),
        "expected solved first replay frame, screen was:\n{screen}"
    );

    session.send_key(b" ")?;
    session.send_key(b"+")?;
    session.send_key(b"q")?;
    let status = session.wait_for_exit(EXIT_TIMEOUT)?;

    assert!(status.success(), "unexpected exit status: {status:?}");
    let output = session.output_string_lossy();
    assert!(
        output.contains("\u{1b}[?1049l"),
        "missing alternate-screen leave sequence"
    );
    assert!(
        output.contains("\u{1b}[?25h"),
        "missing cursor-show sequence"
    );
    Ok(())
}

#[test]
fn live_tcp_tui_reads_recorded_rtcm_in_real_pty() -> TestResult {
    let nav = fixture(&["nav", "KMS300DNK_R_20221591000_01H_MN.rnx"]);
    let rtcm = fixture(&["rtcm", "gmsd7_20121014.rtcm3"]);
    let rtcm_bytes = std::fs::read(&rtcm)?;
    let (port_tx, port_rx) = mpsc::channel();
    let server = thread::spawn(move || -> std::io::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let _ = port_tx.send(port);
        let (mut stream, _) = listener.accept()?;
        loop {
            for chunk in rtcm_bytes.chunks(1024) {
                match stream.write_all(chunk) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                        ) =>
                    {
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                }
                stream.flush()?;
                thread::sleep(Duration::from_millis(2));
            }
        }
    });
    let port = port_rx.recv_timeout(Duration::from_secs(5))?;

    let tcp = format!("127.0.0.1:{port}");
    let mut session = spawn_pty(&[
        "tui",
        "--tcp",
        &tcp,
        "--nav",
        nav.to_str().expect("utf-8 fixture path"),
    ])?;

    let screen = session.wait_for_screen(Duration::from_secs(30), |screen| {
        screen.contains("Solution")
            && screen.contains("tcp connected")
            && screen.contains("epoch 1/")
            && screen.contains("observations ")
    })?;
    assert!(
        screen.contains("G") || screen.contains("C"),
        "expected decoded live satellite rows, screen was:\n{screen}"
    );

    session.send_key(b"q")?;
    let status = session.wait_for_exit(EXIT_TIMEOUT)?;
    assert!(status.success(), "unexpected exit status: {status:?}");
    server.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn tui_arg_matrix_is_exercised_by_real_binary() -> TestResult {
    let obs = fixture(&["obs", "ESBC00DNK_R_20201770000_01D_30S_MO_trim.rnx"]);
    let nav = fixture(&["nav", "ESBC00DNK_R_20201770000_01D_MN.rnx"]);

    let no_args = run_args(&["tui"])?;
    assert_eq!(no_args.status.code(), Some(1));
    assert!(
        no_args.stderr.contains("replay mode requires --obs"),
        "stderr was: {}",
        no_args.stderr
    );

    let ntrip_without_mount = run_args(&[
        "tui",
        "--ntrip",
        "127.0.0.1:2101",
        "--nav",
        nav.to_str().expect("utf-8 fixture path"),
    ])?;
    assert_eq!(ntrip_without_mount.status.code(), Some(1));
    assert!(
        ntrip_without_mount
            .stderr
            .contains("--mount is required for --ntrip"),
        "stderr was: {}",
        ntrip_without_mount.stderr
    );

    let ntrip_and_tcp = run_args(&[
        "tui",
        "--ntrip",
        "127.0.0.1:2101",
        "--tcp",
        "127.0.0.1:2102",
        "--nav",
        nav.to_str().expect("utf-8 fixture path"),
    ])?;
    assert_eq!(ntrip_and_tcp.status.code(), Some(1));
    assert!(
        ntrip_and_tcp
            .stderr
            .contains("exactly one of --ntrip and --tcp is required"),
        "stderr was: {}",
        ntrip_and_tcp.stderr
    );

    let mut replay = spawn_pty(&[
        "tui",
        "--obs",
        obs.to_str().expect("utf-8 fixture path"),
        "--nav",
        nav.to_str().expect("utf-8 fixture path"),
        "--paused",
    ])?;
    let screen = replay.wait_for_screen(SCREEN_TIMEOUT, |screen| {
        screen.contains("Solution") && screen.contains("epoch 1/2")
    })?;
    assert!(
        !replay
            .output_string_lossy()
            .contains("replay mode requires --obs"),
        "valid replay args were rejected, screen was:\n{screen}"
    );
    replay.send_key(b"q")?;
    let status = replay.wait_for_exit(EXIT_TIMEOUT)?;
    assert!(status.success(), "unexpected exit status: {status:?}");

    Ok(())
}

struct CommandOutput {
    status: std::process::ExitStatus,
    stderr: String,
}

fn run_args(args: &[&str]) -> TestResult<CommandOutput> {
    let output = Command::new(binary())
        .args(args)
        .current_dir(workspace_root())
        .output()?;
    Ok(CommandOutput {
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

struct PtySession {
    _master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn Child + Send + Sync>>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    parser: vt100::Parser,
    output: Vec<u8>,
    cursor_report_replies: usize,
}

impl PtySession {
    fn send_key(&mut self, bytes: &[u8]) -> TestResult {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    fn wait_for_screen<F>(&mut self, timeout: Duration, predicate: F) -> TestResult<String>
    where
        F: Fn(&str) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(screen) = self.drain_matching(&predicate) {
                return Ok(screen);
            }
            let screen = self.parser.screen().contents();
            let now = Instant::now();
            if now >= deadline {
                return Err(format!("timed out waiting for screen, last screen:\n{screen}").into());
            }
            let wait = (deadline - now).min(Duration::from_millis(100));
            match self.rx.recv_timeout(wait) {
                Ok(chunk) => {
                    self.process_chunk(&chunk);
                    let screen = self.parser.screen().contents();
                    if predicate(&screen) {
                        return Ok(screen);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!(
                        "pty output ended before expected screen, last screen:\n{screen}"
                    )
                    .into());
                }
            }
        }
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> TestResult<portable_pty::ExitStatus> {
        drop(std::mem::replace(
            &mut self.writer,
            Box::new(std::io::sink()),
        ));
        let mut child = self.child.take().expect("child already waited");
        let mut killer = child.clone_killer();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(child.wait());
        });
        let status = match rx.recv_timeout(timeout) {
            Ok(result) => result?,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = killer.kill();
                rx.recv_timeout(Duration::from_secs(5))
                    .map_err(|_| "child did not exit after kill")??
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("child wait thread disconnected".into());
            }
        };
        self.drain_for(Duration::from_millis(300));
        Ok(status)
    }

    fn output_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }

    fn drain_available(&mut self) {
        while let Ok(chunk) = self.rx.try_recv() {
            self.process_chunk(&chunk);
        }
    }

    fn drain_matching<F>(&mut self, predicate: &F) -> Option<String>
    where
        F: Fn(&str) -> bool,
    {
        let screen = self.parser.screen().contents();
        if predicate(&screen) {
            return Some(screen);
        }
        while let Ok(chunk) = self.rx.try_recv() {
            self.process_chunk(&chunk);
            let screen = self.parser.screen().contents();
            if predicate(&screen) {
                return Some(screen);
            }
        }
        None
    }

    fn drain_for(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(20)) {
                Ok(chunk) => self.process_chunk(&chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        self.drain_available();
    }

    fn process_chunk(&mut self, chunk: &[u8]) {
        self.output.extend_from_slice(chunk);
        let reports = count_subsequence(&self.output, b"\x1b[6n");
        while self.cursor_report_replies < reports {
            let _ = self.writer.write_all(b"\x1b[1;1R");
            let _ = self.writer.flush();
            self.cursor_report_replies += 1;
        }
        self.parser.process(chunk);
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_pty(args: &[&str]) -> TestResult<PtySession> {
    let mut last_error: Option<Box<dyn Error>> = None;
    for _ in 0..2 {
        match spawn_pty_once(args) {
            Ok(session) => return Ok(session),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Err(last_error.expect("spawn attempted"))
}

fn spawn_pty_once(args: &[&str]) -> TestResult<PtySession> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: PTY_ROWS,
        cols: PTY_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new(binary());
    command.args(args);
    command.cwd(workspace_root());
    command.env("TERM", "xterm-256color");
    command.env("RUST_BACKTRACE", "0");

    let child = pair.slave.spawn_command(command)?;
    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    if tx.send(buffer[..size].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(PtySession {
        _master: pair.master,
        child: Some(child),
        writer,
        rx,
        parser: vt100::Parser::new(PTY_ROWS, PTY_COLS, 0),
        output: Vec::new(),
        cursor_report_replies: 0,
    })
}

fn count_subsequence(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_sidereon")
}

fn fixture(parts: &[&str]) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../sidereon-core/tests/fixtures");
    for part in parts {
        path.push(part);
    }
    path
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root")
        .to_path_buf()
}
