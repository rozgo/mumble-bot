use std;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use opus;
use ocbaes128;


#[derive(Clone)]
pub struct Local {
    pub session: u64,
}

pub struct Remote {
    pub session: u64,
    pub sequence: u64,
}

pub type Remotes = HashMap<u64, Box<Remote>>;

pub fn factory(session_id: u64, sessions: Remotes) -> Remotes {
    if sessions.contains_key(&session_id) {
        sessions
    }
    else {
        let session = Box::new(Remote {
            session: session_id,
            sequence: 0,
        });
        let mut sessions = sessions;
        sessions.insert(session_id, session);
        sessions
    }
    // TODO: garbage collect sessions
}

