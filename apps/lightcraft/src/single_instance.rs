//! One window per library: launching the app on a library that's already open brings the open
//! window forward (and imports the files given at launch there) instead of stopping at "This
//! library is already open".
//!
//! The running app listens on a loopback port and records it, with a random token, in
//! `<settings folder>/instances/<pid>.json`. A new launch reads who holds the library from its
//! `catalog.lock.owner`, finds that process's file and sends one line, `{"token", "import",
//! "notices"}`. Only the reply `{"ok": true}` means the open window took it; anything else (no
//! file, nothing listening, a file left by a crash whose port is now someone else's, a wrong
//! token) and the launch carries on as before, to the "already open" message.

use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use serde_json::{Value, json};

/// Longest request line (bytes): a launch with many files is still well under it.
const MAX_LINE: u64 = 4 * 1024 * 1024;
/// How long either side waits for the other.
const TIMEOUT: Duration = Duration::from_secs(3);

/// What a second launch handed over.
#[derive(Debug, Default, PartialEq)]
pub struct Handoff {
    /// Files and folders to import (absolute).
    pub import: Vec<String>,
    /// The second launch's notices (files it refused), shown in the open window.
    pub notices: Vec<String>,
}

/// The listener of the running app: hand-offs arrive on `rx`. Dropping it removes the
/// registration file (the listener thread ends with the process).
pub struct Server {
    pub rx: Receiver<Handoff>,
    file: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

/// Where the process `pid` records its port.
fn registration(config: &Path, pid: u32) -> PathBuf {
    config.join("instances").join(format!("{pid}.json"))
}

/// A token no other program can guess: std's hasher keys are seeded from the OS's randomness.
fn token() -> String {
    let part = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

/// Listen for second launches and record where (`config`: the settings folder). `wake` is called
/// after a hand-off arrives (the window's repaint). None, logged, when it can't listen or record.
pub fn serve(config: &Path, wake: impl Fn() + Send + 'static) -> Option<Server> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| log::warn!("single instance: can't listen: {e}")).ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = token();
    let file = registration(config, std::process::id());
    let written =
        file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, json!({"port": port, "token": token}).to_string()));
    if let Err(e) = written {
        log::warn!("single instance: can't write {}: {e}", file.display());
        return None;
    }
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if answer(stream, &token, &tx) {
                wake();
            }
        }
    });
    Some(Server { rx, file })
}

/// Read one request from `stream`, pass it on and reply; true when it was passed on.
fn answer(mut stream: TcpStream, token: &str, tx: &Sender<Handoff>) -> bool {
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let mut line = String::new();
    let Ok(reader) = stream.try_clone() else { return false };
    if BufReader::new(reader.take(MAX_LINE)).read_line(&mut line).is_err() {
        return false;
    }
    let handoff = parse(&line, token);
    let ok = handoff.is_some_and(|h| tx.send(h).is_ok());
    let reply = if ok { json!({"ok": true}) } else { json!({"ok": false}) };
    let _ = writeln!(stream, "{reply}");
    ok
}

/// The hand-off in `line`, if it is one and carries `token`.
fn parse(line: &str, token: &str) -> Option<Handoff> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("token").and_then(Value::as_str) != Some(token) {
        return None;
    }
    let strings = |key: &str| -> Vec<String> {
        v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
    };
    Some(Handoff { import: strings("import"), notices: strings("notices") })
}

/// Hand `handoff` to the app that has `library` open, if it is one of ours on this computer
/// (`config`: the settings folder). True when it took it: this launch is then done.
pub fn hand_off(library: &Path, config: &Path, handoff: &Handoff) -> bool {
    let owner = std::fs::read(library.join(lightcraft_engine::catalog::lock::OWNER))
        .ok()
        .and_then(|b| serde_json::from_slice::<lightcraft_engine::catalog::LockOwner>(&b).ok());
    let Some(owner) = owner.filter(|o| o.pid != std::process::id()) else { return false };
    let Some(reg) = std::fs::read(registration(config, owner.pid)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
        return false;
    };
    let (Some(port), Some(token)) =
        (reg.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()), reg.get("token").and_then(Value::as_str))
    else {
        return false;
    };
    let request = json!({"token": token, "import": handoff.import, "notices": handoff.notices});
    match send(port, &request.to_string()) {
        Ok(true) => true,
        Ok(false) => {
            log::info!("single instance: process {} didn't take the launch", owner.pid);
            false
        }
        Err(e) => {
            log::info!("single instance: process {} isn't listening ({e})", owner.pid);
            // left by a crash: nobody will answer on it again
            if e.kind() == std::io::ErrorKind::ConnectionRefused {
                let _ = std::fs::remove_file(registration(config, owner.pid));
            }
            false
        }
    }
}

/// Send one request line to the loopback `port`; whether the reply is `{"ok": true}`.
fn send(port: u16, line: &str) -> std::io::Result<bool> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    writeln!(stream, "{line}")?;
    let mut reply = String::new();
    BufReader::new(stream.take(4096)).read_line(&mut reply)?;
    Ok(serde_json::from_str::<Value>(reply.trim()).ok().and_then(|v| v.get("ok").and_then(Value::as_bool)) == Some(true))
}

/// Bring the window forward for a hand-off and import its files.
pub fn receive(app: &mut lightcraft_ui_egui::LightcraftApp, ctx: &egui::Context, handoff: Handoff) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    // where the desktop refuses to raise a window on request, it marks it as wanting attention
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
    app.notices.extend(handoff.notices);
    if handoff.import.is_empty() {
        return;
    }
    match lightcraft_ui_egui::import::start_paths(app, handoff.import) {
        Ok(_) => app.ui.view = lightcraft_ui_egui::state::ViewMode::PhotoGrid,
        Err(e) => app.notices.push(format!("The files opened with {} were not imported: {e}.", lightcraft_ui_egui::APP_NAME)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-single-instance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Pretend `pid` holds `library`.
    fn owned_by(library: &Path, pid: u32) {
        let owner = lightcraft_engine::catalog::LockOwner { pid, ..Default::default() };
        std::fs::write(library.join(lightcraft_engine::catalog::lock::OWNER), serde_json::to_vec(&owner).unwrap()).unwrap();
    }

    #[test]
    fn a_second_launch_reaches_the_open_window() {
        let (config, library) = (temp("config"), temp("library"));
        let server = serve(&config, || {}).unwrap();
        // the open window is this process; the launch looks for the owner's registration
        let reg = registration(&config, std::process::id());
        assert!(reg.exists());
        let other = 4_000_000_000;
        std::fs::copy(&reg, registration(&config, other)).unwrap();
        owned_by(&library, other);
        let sent = Handoff { import: vec!["/photos/a.jpg".into()], notices: vec!["n".into()] };
        assert!(hand_off(&library, &config, &sent));
        assert_eq!(server.rx.recv_timeout(Duration::from_secs(5)).unwrap(), sent);
        drop(server);
        assert!(!reg.exists(), "the registration goes with the server");
    }

    #[test]
    fn a_wrong_token_or_stale_file_is_refused() {
        let (config, library) = (temp("stale-config"), temp("stale-library"));
        let server = serve(&config, || {}).unwrap();
        let reg: Value = serde_json::from_slice(&std::fs::read(registration(&config, std::process::id())).unwrap()).unwrap();
        let other = 4_000_000_001;
        std::fs::write(registration(&config, other), json!({"port": reg["port"], "token": "guess"}).to_string()).unwrap();
        owned_by(&library, other);
        assert!(!hand_off(&library, &config, &Handoff::default()));
        assert!(server.rx.try_recv().is_err());
        // nothing listening on the recorded port
        let closed = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        std::fs::write(registration(&config, other), json!({"port": closed, "token": "x"}).to_string()).unwrap();
        assert!(!hand_off(&library, &config, &Handoff::default()));
        assert!(!registration(&config, other).exists(), "a file nobody answers on is removed");
    }

    #[test]
    fn junk_owner_or_registration_files_mean_no_hand_off() {
        let (config, library) = (temp("junk-config"), temp("junk-library"));
        assert!(!hand_off(&library, &config, &Handoff::default()), "no owner file");
        std::fs::write(library.join(lightcraft_engine::catalog::lock::OWNER), b"\xff not json").unwrap();
        assert!(!hand_off(&library, &config, &Handoff::default()));
        owned_by(&library, 4_000_000_002);
        std::fs::create_dir_all(config.join("instances")).unwrap();
        std::fs::write(registration(&config, 4_000_000_002), json!({"port": 99_999_999, "token": 3}).to_string()).unwrap();
        assert!(!hand_off(&library, &config, &Handoff::default()));
        assert_eq!(parse("{\"token\":\"t\",\"import\":[1,\"a\"]}", "t"), Some(Handoff { import: vec!["a".into()], notices: vec![] }));
        assert_eq!(parse("GET / HTTP/1.1", "t"), None);
    }
}
