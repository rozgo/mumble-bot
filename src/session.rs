use std;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use opus;
use ocbaes128;


#[derive(Clone)]
pub struct Local {
    pub session: u64,
    pub crypt_state: Arc<Mutex<ocbaes128::CryptState>>,
}

pub struct Remote {
    pub decoder: Box<opus::Decoder>,
    pub session: u64,
    pub sequence: u64,
}

pub type Remotes = HashMap<u64, Box<Remote>>;

pub fn factory(session_id: u64, sessions: Arc<Mutex<HashMap<u64, std::boxed::Box<Remote>>>>)
    -> Arc<Mutex<HashMap<u64, std::boxed::Box<Remote>>>> {
    {
        let sessions = sessions.lock().unwrap();
        if !sessions.contains_key(&session_id) {
            let session = Box::new(Remote {
                decoder: Box::new(opus::Decoder::new(16000, opus::Channels::Mono).unwrap()),
                session: session_id,
                sequence: 0,
            });
            let mut sessions = sessions;
            sessions.insert(session_id, session);
        }
    }
    sessions
    // TODO: garbage collect sessions
}

