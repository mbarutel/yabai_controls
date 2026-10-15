//! Round-trips the yabai wire protocol against a fake daemon on a temp socket.

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::thread;

use yabai_controls::yabai::{Client, encode_message};

/// Spawns a one-shot fake yabai that records the request and replies with
/// `reply`. Returns the client and a handle yielding the raw request bytes.
fn fake_yabai(reply: &'static [u8]) -> (Client, tempfile::TempDir, thread::JoinHandle<Vec<u8>>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("yabai.socket");
    let listener = UnixListener::bind(&path).unwrap();

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        stream.read_to_end(&mut request).unwrap(); // client half-closes
        stream.write_all(reply).unwrap();
        request
    });

    (Client::new(path), dir, handle)
}

#[test]
fn encodes_length_prefix_and_nul_separated_args() {
    let bytes = encode_message(&["space", "--focus", "3"]);
    // payload = "space\0--focus\03\0" + trailing "\0" = 6 + 8 + 2 + 1 = 17
    let expected_len: i32 = 17;
    assert_eq!(&bytes[..4], &expected_len.to_ne_bytes());
    assert_eq!(&bytes[4..], b"space\0--focus\x003\0\0");
}

#[test]
fn sends_request_and_reads_reply_to_eof() {
    let (client, _dir, server) = fake_yabai(b"{\"id\":1}\n");

    let reply = client.send(&["query", "--windows", "--window"]).unwrap();
    assert_eq!(reply, "{\"id\":1}\n");

    let request = server.join().unwrap();
    assert_eq!(request, encode_message(&["query", "--windows", "--window"]));
}

#[test]
fn empty_reply_is_success() {
    let (client, _dir, server) = fake_yabai(b"");
    assert_eq!(client.send(&["space", "--focus", "1"]).unwrap(), "");
    server.join().unwrap();
}

#[test]
fn bel_prefixed_reply_is_an_error_with_message() {
    let (client, _dir, server) =
        fake_yabai(b"\x07could not locate space with the given selector\n");
    let err = client.send(&["space", "--focus", "99"]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "yabai: could not locate space with the given selector"
    );
    server.join().unwrap();
}

#[test]
fn missing_socket_is_a_connect_error() {
    let client = Client::new("/tmp/yabai_controls_test_does_not_exist.socket");
    let err = client.send(&["query", "--spaces"]).unwrap_err();
    assert!(
        err.to_string().contains("connecting to yabai socket"),
        "{err}"
    );
}
