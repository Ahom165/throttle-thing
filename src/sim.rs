//! Mode simulation : quand le driver n'est pas disponible (essais, autre OS,
//! démonstrations), deux threads génèrent des I/O bridées par le bucket à
//! jetons Rust — mêmes mathématiques que le driver C — pour visualiser le
//! comportement exact de la limitation sans rien toucher au disque.

use crate::token_bucket::TokenBucket;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Compteurs partagés lus par l'interface (octets cumulés).
#[derive(Clone, Default)]
pub struct SimCounters {
    pub total_read: Arc<AtomicU64>,
    pub total_write: Arc<AtomicU64>,
}

/// Poignée de la simulation : `stop()` consomme la valeur et joint les threads.
pub struct SimHandle {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl SimHandle {
    /// Démarre un flux lecture + un flux écriture plafonnés aux débits donnés
    /// (0 = illimité côté simulation : on émet alors au rythme des ticks).
    pub fn start(read_bps: u64, write_bps: u64, counters: SimCounters) -> SimHandle {
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();

        for (rate, counter) in [
            (read_bps, counters.total_read.clone()),
            (write_bps, counters.total_write.clone()),
        ] {
            let stop = stop.clone();
            threads.push(std::thread::spawn(move || {
                let mut bucket = TokenBucket::new(rate);
                let mut state = 0x9E37_79B9_7F4A_7C15u64; // PRNG xorshift, déterministe
                while !stop.load(Ordering::Relaxed) {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    // requêtes de 4 Kio à 512 Kio, comme de vraies I/O fichiers
                    let size =
                        4 * 1024 + (state % (512 * 1024 - 4 * 1024 + 1)) as u64;
                    bucket.acquire(size);
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    counter.fetch_add(size, Ordering::Relaxed);
                    if rate == 0 {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            }));
        }

        SimHandle { stop, threads }
    }

    /// Arrête les threads et attend leur fin.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
