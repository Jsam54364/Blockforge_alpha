//! Client <-> server messages and the in-process transport.
//!
//! Singleplayer runs the real server on a thread and talks to it through
//! `local_pair()`. A network transport would implement the same `send`/`try_recv`
//! shape and encode chunks with `save::encode_chunk` (already versioned + checksummed).

use std::sync::mpsc;
use std::time::Duration;

use crate::block::BlockState;
use crate::chunk::Chunk;
use crate::coords::{BlockPos, ChunkPos};

#[derive(Debug)]
pub enum ClientMsg {
    Hello { name: String },
    /// Player position, used for chunk streaming and simulation distance.
    Position { pos: [f32; 3] },
    SetViewDistance { chunks: u8 },
    BreakBlock { pos: BlockPos },
    PlaceBlock { pos: BlockPos, state: BlockState },
    Undo,
    Redo,
    Disconnect,
}

#[derive(Debug)]
pub enum ServerMsg {
    /// `blocks_toml` is the server's block registry, so numeric ids always match.
    /// `vertical_view_chunks` is the server's vertical streaming cap, so the client can tell
    /// which neighbours of a chunk are still on their way (see `Client::neighbor_wait`).
    Welcome { seed: u64, tick_hz: u32, vertical_view_chunks: i32, blocks_toml: String },
    ChunkData { pos: ChunkPos, chunk: Box<Chunk> },
    ChunkUnload { pos: ChunkPos },
    BlockUpdate { pos: BlockPos, state: BlockState },
}

/// One end of a bidirectional channel: sends `S`, receives `R`.
pub struct Endpoint<S, R> {
    tx: mpsc::Sender<S>,
    rx: mpsc::Receiver<R>,
}

impl<S, R> Endpoint<S, R> {
    /// Returns false if the other side is gone.
    pub fn send(&self, msg: S) -> bool {
        self.tx.send(msg).is_ok()
    }

    pub fn try_recv(&self) -> Result<R, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    pub fn recv_timeout(&self, d: Duration) -> Result<R, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(d)
    }
}

pub fn local_pair() -> (Endpoint<ClientMsg, ServerMsg>, Endpoint<ServerMsg, ClientMsg>) {
    let (c2s_tx, c2s_rx) = mpsc::channel();
    let (s2c_tx, s2c_rx) = mpsc::channel();
    (Endpoint { tx: c2s_tx, rx: s2c_rx }, Endpoint { tx: s2c_tx, rx: c2s_rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_flow_both_ways_and_disconnect_is_detected() {
        let (c, s) = local_pair();
        assert!(c.send(ClientMsg::Undo));
        assert!(matches!(s.try_recv(), Ok(ClientMsg::Undo)));
        assert!(s.send(ServerMsg::ChunkUnload { pos: ChunkPos::new(1, 2, 3) }));
        assert!(matches!(c.try_recv(), Ok(ServerMsg::ChunkUnload { .. })));
        assert!(matches!(c.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(s);
        assert!(matches!(c.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
        assert!(!c.send(ClientMsg::Undo));
    }
}
