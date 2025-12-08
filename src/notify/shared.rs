use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, LazyLock, Mutex as StdMutex},
};

use tokio::sync::Mutex as TokioMutex;

use crate::{notify::NotifierTrait, platform::PlatformTraitStatic};

pub trait NotifierShared: Default {
    type Notifier: NotifierTrait + PlatformTraitStatic;
    type ConfigParams;

    fn params_key(params: &Self::ConfigParams) -> String;
}

// Cross-notifier shared states manager
//
// Because each subscription instantiates a separate notifier, some states may
// need to be shared across different subscriptions with the same destination
// (such as the same recipient).
pub struct SharedManager<S>(LazyLock<StdMutex<HashMap<String, Arc<TokioMutex<S>>>>>);

impl<S: NotifierShared> SharedManager<S> {
    pub const fn new() -> Self {
        Self(LazyLock::new(|| StdMutex::new(HashMap::new())))
    }

    pub fn obtain(&self, params: &S::ConfigParams) -> Arc<TokioMutex<S>> {
        let key = format!(
            "{}:{}",
            S::Notifier::metadata().display_name,
            S::params_key(params)
        );

        match self.0.lock().unwrap().entry(key) {
            Entry::Occupied(entry) => Arc::clone(entry.get()),
            Entry::Vacant(entry) => {
                let state = Arc::new(TokioMutex::new(S::default()));
                entry.insert(Arc::clone(&state));
                state
            }
        }
    }
}
