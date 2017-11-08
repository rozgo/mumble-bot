use std;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use opus;
use ocbaes128;


#[derive(Clone)]
pub struct Local {
    pub id: u64,
}

pub struct Remote {
    pub id: u64,
}

pub struct Session {
    pub local: Local,
    pub remotes: HashMap<u64, Remote>,
}

