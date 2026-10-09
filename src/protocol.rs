//! Protocole applicatif <-> minifilter.
//!
//! Mise en correspondance EXACTE avec `driver-windows/throttle.h` (packing
//! natif x64, pas de `#pragma pack`). Toute modification ici doit être
//! répercutée dans le header C — les `C_ASSERT` côté C et les `const assert`
//! côté Rust feront échouer la compilation en cas de divergence.

pub const CMD_SET_CONFIG: u32 = 1;
pub const CMD_CLEAR: u32 = 2;
pub const CMD_GET_STATS: u32 = 3;

/// MAX_PATH Windows.
pub const PATH_CHARS: usize = 260;

/// Configuration envoyée par l'application au driver.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ThrottleConfig {
    /// 1 = limitation active, 0 = inactive.
    pub enabled: u32,
    pub _pad0: u32,
    /// Débit lecture autorisé en octets/s. 0 = illimité.
    pub read_bps: u64,
    /// Débit écriture autorisé en octets/s. 0 = illimité.
    pub write_bps: u64,
    /// PID du processus à exclure (l'application elle-même).
    pub app_pid: u64,
    /// Chemin du dossier en forme device (« \Device\HarddiskVolume3\dir\ »),
    /// terminé par nul.
    pub target_path: [u16; PATH_CHARS],
}

impl Default for ThrottleConfig {
    fn default() -> Self {
        ThrottleConfig {
            enabled: 0,
            _pad0: 0,
            read_bps: 0,
            write_bps: 0,
            app_pid: 0,
            target_path: [0; PATH_CHARS],
        }
    }
}

/// Message envoyé via FilterSendMessage.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ThrottleMessage {
    /// CMD_SET_CONFIG / CMD_CLEAR / CMD_GET_STATS.
    pub command: u32,
    pub _pad0: u32,
    pub config: ThrottleConfig,
}

/// Réponse du driver à CMD_GET_STATS (rempli dans le tampon de réponse).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ThrottleStats {
    pub total_read: u64,
    pub total_write: u64,
    /// Débit mesuré depuis le dernier GET_STATS.
    pub read_bps: u64,
    pub write_bps: u64,
    /// 1 = limitation active côté driver.
    pub active: u32,
    pub _pad0: u32,
}

// Garde-fous de layout (doivent refléter les C_ASSERT de throttle.h) :
// - ThrottleConfig  : 4+4 + 8 + 8 + 8 + 260*2 = 552
// - ThrottleMessage : 4+4 + 552 = 560
// - ThrottleStats   : 8*4 + 4+4 = 40
const _: () = assert!(std::mem::size_of::<ThrottleConfig>() == 552);
const _: () = assert!(std::mem::size_of::<ThrottleMessage>() == 560);
const _: () = assert!(std::mem::size_of::<ThrottleStats>() == 40);

impl ThrottleConfig {
    /// Écrit `path` dans le champ `target_path` (UTF-16, terminé par nul).
    pub fn set_path(&mut self, path: &str) {
        self.target_path = [0; PATH_CHARS];
        let units: Vec<u16> = path.encode_utf16().take(PATH_CHARS - 1).collect();
        self.target_path[..units.len()].copy_from_slice(&units);
    }

    /// Relit le chemin stocké (diagnostique).
    pub fn path(&self) -> String {
        let end = self
            .target_path
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(PATH_CHARS);
        String::from_utf16_lossy(&self.target_path[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aller_retour_chemin() {
        let mut cfg = ThrottleConfig::default();
        cfg.set_path(r"\Device\HarddiskVolume3\Users\moi\Dossier");
        assert_eq!(cfg.path(), r"\Device\HarddiskVolume3\Users\moi\Dossier");
    }

    #[test]
    fn chemin_trop_long_tronque_sans_panic() {
        let mut cfg = ThrottleConfig::default();
        let long = "A".repeat(1000);
        cfg.set_path(&long);
        assert_eq!(cfg.path().chars().count(), PATH_CHARS - 1);
    }

    #[test]
    fn layouts_stables() {
        assert_eq!(std::mem::size_of::<ThrottleConfig>(), 552);
        assert_eq!(std::mem::size_of::<ThrottleMessage>(), 560);
        assert_eq!(std::mem::size_of::<ThrottleStats>(), 40);
    }
}
