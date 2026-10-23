use super::*;
use aquilum_gateway::auth;
use serde_json::json;
use std::{
    fs,
    time::{Duration, Instant},
};

fn core(data: &Path, root: &Path) -> Arc<Core> {
    fs::create_dir_all(data).unwrap();
    fs::create_dir_all(root).unwrap();
    let core = Core::open(
        data,
        Arc::new(|event| {
            if let CoreEvent::Settle(done) = event {
                let _ = done.send(());
            }
        }),
    );
    core.ui_state
        .resolve_workspace(&root.to_string_lossy(), 0)
        .unwrap();
    core
}

fn wait(core: &Core) -> Status {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let s = core.connections.status();
        if !s.busy {
            return s;
        }
        assert!(Instant::now() < deadline, "connection did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Pair {
    _dir: tempfile::TempDir,
    host: Arc<Core>,
    guest: Arc<Core>,
    root: PathBuf,
    local: PathBuf,
    token: PathBuf,
}

impl Pair {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("remote");
        let local = dir.path().join("local");
        let data = dir.path().join("host");
        fs::create_dir_all(data.join("client-api")).unwrap();
        fs::create_dir(&root).unwrap();
        let token = dir.path().join("guest.token");
        auth::create_token(&data.join("client-api/tokens.json"), "guest", &token).unwrap();
        fs::write(data.join("client-api/config.json"), json!({
            "enabled": true, "bind": "127.0.0.1:0", "workspace": root,
            "tokens_file": "tokens.json", "allowed_origins": [], "requests_per_minute": 120, "burst": 20,
        }).to_string()).unwrap();
        let host = core(&data, &root);
        let guest = core(&dir.path().join("guest"), &local);
        guest.connections.configure(|c| {
            c.address = format!("http://{}/api", host.client_api.status().address.unwrap());
            c.token_file = Some(token.clone());
        });
        Self {
            _dir: dir,
            host,
            guest,
            root,
            local,
            token,
        }
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        self.guest.shutdown();
        self.host.shutdown();
    }
}

#[test]
fn connections_browse_read_refresh_copy_complete_unicode_notes_and_revoke_access() {
    let p = Pair::new();
    fs::create_dir(p.root.join("Проекты")).unwrap();
    let content = format!(
        "---\ntags: [test]\n---\n# Пример\n{}",
        "Длинная заметка 🚀\n".repeat(5000)
    );
    fs::write(p.root.join("Проекты/Пример.md"), &content).unwrap();
    fs::write(p.root.join("Empty.md"), "").unwrap();
    fs::write(p.local.join("Empty.md"), "keep local work").unwrap();
    p.guest.connections.start(&p.guest, Job::Browse(0));
    let s = wait(&p.guest);
    assert!(s.connected, "{:?}", s.error);
    assert_eq!(s.total, 2);
    p.guest
        .connections
        .start(&p.guest, Job::Read("Проекты/Пример.md".into(), 0));
    let s = wait(&p.guest);
    assert_eq!(s.preview.as_ref().unwrap().content.chars().count(), 4000);
    let next = s.preview.unwrap().next.unwrap();
    p.guest
        .connections
        .start(&p.guest, Job::Read("Проекты/Пример.md".into(), next));
    assert_eq!(wait(&p.guest).preview_offset, 4000);
    p.guest.connections.start(
        &p.guest,
        Job::Copy {
            workspace: p.local.clone(),
            note: None,
        },
    );
    let s = wait(&p.guest);
    assert!(s.error.is_none(), "{:?}", s.error);
    assert_eq!(s.copied, 2);
    let imported = s.destination.unwrap();
    assert_eq!(
        fs::read_to_string(imported.join("Проекты/Пример.md")).unwrap(),
        content
    );
    assert_eq!(fs::read_to_string(imported.join("Empty.md")).unwrap(), "");
    assert_eq!(
        fs::read_to_string(p.local.join("Empty.md")).unwrap(),
        "keep local work"
    );
    fs::write(p.root.join("Empty.md"), "latest remote text").unwrap();
    p.guest
        .connections
        .start(&p.guest, Job::Read("Empty.md".into(), 0));
    assert_eq!(
        wait(&p.guest).preview.unwrap().content,
        "latest remote text"
    );
    p.guest.connections.start(
        &p.guest,
        Job::Copy {
            workspace: p.local.clone(),
            note: Some("Empty.md".into()),
        },
    );
    let s = wait(&p.guest);
    assert_eq!(s.copied, 1);
    assert_ne!(s.destination.as_ref().unwrap(), &imported);
    assert_eq!(
        fs::read_to_string(s.destination.unwrap().join("Empty.md")).unwrap(),
        "latest remote text"
    );
    let saved = fs::read_to_string(&p.guest.connections.path).unwrap();
    assert!(!saved.contains(&fs::read_to_string(&p.token).unwrap()));
    p.host.client_api.revoke_token("guest").unwrap();
    p.guest.connections.start(&p.guest, Job::Browse(0));
    let s = wait(&p.guest);
    assert!(!s.connected);
    assert!(s.error.unwrap().contains("revoked"));
}

#[test]
fn remote_listing_paginates_filters_and_invalid_requests_do_not_write() {
    let p = Pair::new();
    for i in 0..23 {
        fs::write(p.root.join(format!("Note-{i:02}.md")), "hello").unwrap();
    }
    p.guest.connections.start(&p.guest, Job::Browse(0));
    let s = wait(&p.guest);
    assert_eq!(s.notes.len(), 20);
    assert_eq!(s.next, Some(20));
    p.guest.connections.start(&p.guest, Job::Browse(20));
    let s = wait(&p.guest);
    assert_eq!(s.notes.len(), 3);
    assert!(s.next.is_none());
    fs::write(p.root.join("100%.md"), "not exposed by the API").unwrap();
    p.guest.connections.start(&p.guest, Job::Browse(0));
    let s = wait(&p.guest);
    assert!(s.error.is_none(), "{:?}", s.error);
    assert_eq!(s.notes.len(), 19);
    assert_eq!(s.next, Some(20));
    p.guest.connections.query("Note-22".into());
    p.guest.connections.start(&p.guest, Job::Browse(0));
    assert_eq!(wait(&p.guest).notes, ["Note-22.md"]);
    p.guest.connections.start(
        &p.guest,
        Job::Copy {
            workspace: p.local.clone(),
            note: Some("../escape.md".into()),
        },
    );
    assert!(wait(&p.guest).error.is_some());
    assert_eq!(fs::read_dir(&p.local).unwrap().count(), 0);
    p.guest.connections.close();
    p.guest.connections.start(&p.guest, Job::Browse(0));
    assert!(!p.guest.connections.status().busy);
}

#[test]
fn addresses_and_remote_paths_cannot_leak_credentials_or_escape_imports() {
    for url in [
        "http://example.com/api",
        "https://user:secret@example.com/api",
        "https://example.com/api?token=x",
        "https://example.com/api#fragment",
        "file:///tmp/api",
    ] {
        assert!(client::address(url).is_err(), "{url}");
    }
    for url in [
        "https://example.com/notes-api",
        "http://127.0.0.1:8790/api",
        "http://[::1]:8790/api",
    ] {
        assert!(client::address(url).is_ok(), "{url}");
    }
    for path in [
        "../escape.md",
        "/absolute.md",
        "C:/file.md",
        ".aquilum/file.md",
        "dir/../escape.md",
        "dir\\escape.md",
        "CON.md",
        "dir/Lpt1.md",
        "dir /file.md",
        "dir//file.md",
        "dir/file.md:stream",
    ] {
        assert!(client::note_path(path).is_err(), "{path}");
    }
    assert!(client::note_path("Проекты/Пример.md").is_ok());
    assert!(validate_listing(
        &client::Listing {
            notes: vec![],
            total: 20,
            next: Some(0)
        },
        0
    )
    .is_err());
    assert!(validate_listing(
        &client::Listing {
            notes: vec!["safe.md".into()],
            total: 20,
            next: Some(0)
        },
        0
    )
    .is_err());
}

#[test]
fn cancel_during_download_does_not_apply_the_response_or_allow_another_job() {
    use std::io::{Read, Write};
    let dir = tempfile::tempdir().unwrap();
    let guest = core(&dir.path().join("data"), &dir.path().join("local"));
    let token = dir.path().join("client.token");
    fs::write(&token, format!("aqg_{}", "ab".repeat(32))).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    guest.connections.configure(|c| {
        c.address = format!("http://{}/api", listener.local_addr().unwrap());
        c.token_file = Some(token);
    });
    let (received, requested) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.extend(byte);
        }
        received.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(5)).unwrap();
        let body = r#"{"notes":["Example.md"],"total":1,"nextOffset":null}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    guest.connections.start(&guest, Job::Browse(0));
    requested.recv_timeout(Duration::from_secs(5)).unwrap();
    guest.connections.query("ignored while busy".into());
    guest.connections.start(&guest, Job::Browse(20));
    guest.connections.disconnect();
    release.send(()).unwrap();
    let status = wait(&guest);
    assert!(!status.connected);
    assert!(status.notes.is_empty());
    assert!(status.query.is_empty());
    assert!(status.error.unwrap().contains("cancelled"));
    server.join().unwrap();
    guest.shutdown();
}

#[test]
fn oversized_snapshot_is_refused_without_creating_a_truncated_note() {
    let p = Pair::new();
    fs::write(p.root.join("Large.md"), vec![b'x'; 8 * 1024 * 1024 + 1]).unwrap();
    p.guest.connections.start(
        &p.guest,
        Job::Copy {
            workspace: p.local.clone(),
            note: Some("Large.md".into()),
        },
    );
    let s = wait(&p.guest);
    assert!(s.error.is_some());
    assert_eq!(s.copied, 0);
    assert!(!s.destination.unwrap().join("Large.md").exists());
}
