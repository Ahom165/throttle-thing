//! Bucket à jetons (token bucket) — miroir Rust de l'algorithme implémenté
//! côté driver en C (`driver-windows/throttle.c`). Sert aux tests unitaires
//! et au mode simulation de l'interface : mêmes mathématiques, deux mondes.

use std::time::{Duration, Instant};

/// Bucket à jetons mono-direction (lecture OU écriture).
///
/// - `rate_bps == 0` signifie « illimité » : tout passe immédiatement.
/// - La capacité vaut au minimum 64 Kio et au maximum 1 s de débit : c'est la
///   rafale autorisée avant que le pontage ne s'applique.
pub struct TokenBucket {
    rate_bps: u64,
    capacity: u64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(rate_bps: u64) -> Self {
        let mut b = TokenBucket {
            rate_bps: 0,
            capacity: 0,
            tokens: 0.0,
            last: Instant::now(),
        };
        b.set_rate(rate_bps);
        b
    }

    pub fn rate(&self) -> u64 {
        self.rate_bps
    }

    pub fn set_rate(&mut self, rate_bps: u64) {
        self.rate_bps = rate_bps;
        self.capacity = rate_bps.max(64 * 1024);
        self.tokens = self.capacity as f64;
        self.last = Instant::now();
    }

    /// Recharge les jetons pour la durée écoulée (bornée à 2 s, comme côté C,
    /// pour éviter tout débordement après une longue inactivité).
    pub fn refill(&mut self, dt: Duration) {
        if self.rate_bps == 0 {
            return;
        }
        let dt = dt.min(Duration::from_secs(2));
        self.tokens = (self.tokens + dt.as_secs_f64() * self.rate_bps as f64)
            .min(self.capacity as f64);
    }

    /// Consomme `bytes` jetons si disponibles, sinon renvoie la durée
    /// minimale à attendre avant que l'opération puisse aboutir.
    pub fn take(&mut self, bytes: u64) -> Duration {
        if self.rate_bps == 0 {
            return Duration::ZERO;
        }
        self.refill(self.last.elapsed());
        self.last = Instant::now();
        if (bytes as f64) <= self.tokens {
            self.tokens -= bytes as f64;
            Duration::ZERO
        } else {
            let needed = (bytes as f64) - self.tokens;
            let secs = needed / self.rate_bps as f64;
            Duration::from_secs_f64(secs.max(0.001))
        }
    }

    /// Bloque jusqu'à pouvoir consommer `bytes` (boucle consommer/attendre —
    /// c'est exactement ce que fait le driver avec KeDelayExecutionThread).
    pub fn acquire(&mut self, bytes: u64) {
        if self.rate_bps == 0 || bytes == 0 {
            return;
        }
        loop {
            self.refill(self.last.elapsed());
            self.last = Instant::now();
            if (bytes as f64) <= self.tokens {
                self.tokens -= bytes as f64;
                return;
            }
            let needed = (bytes as f64) - self.tokens;
            let secs = (needed / self.rate_bps as f64).clamp(0.001, 1.0);
            std::thread::sleep(Duration::from_secs_f64(secs));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn illimite_ne_bloque_pas() {
        let mut b = TokenBucket::new(0);
        assert_eq!(b.take(u64::MAX), Duration::ZERO);
        b.acquire(u64::MAX);
    }

    #[test]
    fn rafale_initiale_puis_attente() {
        let mut b = TokenBucket::new(1_000_000); // 1 Mo/s
        // rafale complète disponible immédiatement
        assert_eq!(b.take(1_000_000), Duration::ZERO);
        // bucket vide : 1 Mo de plus -> ~1 s d'attente
        let wait = b.take(1_000_000);
        assert!(wait >= Duration::from_millis(900), "attente trop courte : {:?}", wait);
        assert!(wait <= Duration::from_millis(1100), "attente trop longue : {:?}", wait);
    }

    #[test]
    fn recharge_est_proportionnelle_au_temps() {
        let mut b = TokenBucket::new(2_000_000); // 2 Mo/s
        b.tokens = 0.0;
        b.refill(Duration::from_millis(500)); // 0,5 s -> 1 Mo
        assert!((b.tokens - 1_000_000.0).abs() < 1.0);
        b.refill(Duration::from_secs(10)); // borné à 2 s -> capacité 2 Mo
        assert!((b.tokens - 2_000_000.0).abs() < 1.0);
    }

    #[test]
    fn la_capacite_vaut_au_moins_64_kio() {
        let mut b = TokenBucket::new(10); // 10 o/s
        assert_eq!(b.capacity, 64 * 1024);
        assert!(b.take(64 * 1024) == Duration::ZERO);
        // au-delà de la capacité même en rafale -> attente
        assert!(b.take(64 * 1024) > Duration::ZERO);
    }

    #[test]
    fn debit_respecte_la_limite() {
        // Consommation continue de 200 Ko par unité : à 100 Ko/s,
        // deux unités pleines nécessitent ~1 s entre elles.
        let mut b = TokenBucket::new(100 * 1024);
        let t0 = std::time::Instant::now();
        for _ in 0..3 {
            b.acquire(100 * 1024);
        }
        let elapsed = t0.elapsed();
        // 2 recharges de ~1 s minimum (la première unité part de la rafale)
        assert!(elapsed >= Duration::from_millis(1500), "trop rapide : {:?}", elapsed);
    }
}
