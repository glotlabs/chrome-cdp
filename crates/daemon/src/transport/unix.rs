//! The session socket: a Unix socket, mode 0600, in the session's 0700
//! directory. The only Unix-socket code in the daemon (overview §8).

use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use tokio::net::{UnixListener, UnixStream};

/// The server's end of a connection.
pub(crate) type Stream = UnixStream;

/// Binds the session socket, replacing a stale one left by a dead daemon
/// (the caller holds the daemon lock, so no live daemon owns it).
pub(crate) fn bind(path: &Path) -> io::Result<UnixListener> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Connects to a session socket.
pub(crate) async fn connect(path: &Path) -> io::Result<UnixStream> {
    UnixStream::connect(path).await
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    #[tokio::test]
    async fn the_socket_is_private_and_replaces_a_stale_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sock");
        std::fs::write(&path, "stale").unwrap();
        let listener = super::bind(&path).unwrap();
        let mode = path.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let (client, server) = tokio::join!(super::connect(&path), listener.accept());
        client.unwrap();
        server.unwrap();
    }
}
