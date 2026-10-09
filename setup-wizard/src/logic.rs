//! Logique de l'assistant : actions à exécuter, exécution des commandes
//! système, détection de l'état. Testable hors Windows (commandes stubbées).

// ---- Fichiers embarqués dans l'exécutable --------------------------------

pub const SYS_BYTES: &[u8] = include_bytes!("../embed/throttle.sys");
pub const CER_BYTES: &[u8] = include_bytes!("../embed/throttle-test.cer");

// ---- Constantes d'installation -------------------------------------------

pub const SVC: &str = "throttle";
pub const INSTALL_DIR: &str = r"C:\ProgramData\ThrottleFolder";
pub const SYS_PATH: &str = r"C:\ProgramData\ThrottleFolder\throttle.sys";
pub const CER_PATH: &str = r"C:\ProgramData\ThrottleFolder\throttle-test.cer";
pub const DISPLAY_NAME: &str = "Limiteur de debit dossier (minifilter)";
pub const DESCRIPTION: &str = "Limite le debit lecture/ecriture d'un dossier pour tous les processus";
pub const INSTANCE_KEY: &str = "throttle - Default Instance";

/// Clé de l'« Intégrité de la mémoire » (HVCI) — Windows 11 l'active par défaut
/// et BLOQUE les drivers test-signés tant qu'elle est active, même en mode test.
pub const HVCI_KEY: &str =
    r"HKLM\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity";
/// Ancienne clé (builds antérieurs) utilisée en repli.
pub const HVCI_KEY_OLD: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\DeviceGuard";

/// Clé RunOnce : lance automatiquement l'assistant après le redémarrage pour
/// terminer l'installation sans intervention (l'utilisateur n'accepte que l'UAC).
pub const RUNONCE_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce";
pub const RUNONCE_VALUE: &str = "ThrottleSetup";

// ---- Actions --------------------------------------------------------------

/// Une étape concrète de l'assistant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Détecte l'état courant (mode test, service installé).
    CheckState,
    /// Écrit le certificat de test et l'installe (Racine + Éditeurs approuvés).
    InstallCert,
    /// Active le mode test-signature (bcdedit) — nécessite un redémarrage.
    EnableTestsigning,
    /// Copie throttle.sys dans C:\ProgramData\ThrottleFolder.
    DeploySys,
    /// Crée le service minifilter + enregistre l'instance (altitude).
    CreateService,
    /// Charge le filtre (fltmc load).
    LoadFilter,
    /// Désactive l'« Intégrité de la mémoire » (HVCI) — Windows 11 la bloque
    /// pour les drivers test-signés, même en mode test.
    DisableHvci,
    /// Programme la relance automatique de l'assistant après redémarrage
    /// (RunOnce) — sans effet si aucun redémarrage n'est requis.
    ScheduleRelaunch,
    /// Décharge le filtre (fltmc unload).
    UnloadFilter,
    /// Supprime le service et ses clés de registre.
    DeleteService,
    /// Supprime les fichiers déployés.
    RemoveFiles,
}

/// Liste des actions d'installation, selon l'état détecté.
pub fn install_actions(testsigning: Option<bool>, hvci: Option<bool>) -> Vec<Action> {
    let mut v = vec![Action::InstallCert];
    if testsigning != Some(true) {
        v.push(Action::EnableTestsigning);
    }
    v.push(Action::DeploySys);
    v.push(Action::CreateService);
    if hvci == Some(true) {
        v.push(Action::DisableHvci);
    }
    v.push(Action::LoadFilter);
    v.push(Action::ScheduleRelaunch);
    v
}

// ---- Exécution des commandes ----------------------------------------------

#[cfg(windows)]
pub fn run_cmd(cmd: &str, args: &[&str]) -> (bool, String) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    match std::process::Command::new(cmd)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(out) => {
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&out.stdout));
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            (out.status.success(), text)
        }
        Err(e) => (false, format!("impossible de lancer {cmd} : {e}")),
    }
}

#[cfg(not(windows))]
pub fn run_cmd(_cmd: &str, _args: &[&str]) -> (bool, String) {
    (false, String::from("[hors Windows : commande non executee]"))
}

// ---- Analyse de l'état -----------------------------------------------------

/// Détermine si le mode test-signature est actif, à partir de la sortie de
/// `bcdedit /enum {current}`.
///
/// La valeur est localisée (« Yes » / « Oui » / « No » / « Non ») mais le nom
/// de la clé `testsigning` reste en anglais.
pub fn parse_testsigning(bcdedit_out: &str) -> Option<bool> {
    for line in bcdedit_out.lines() {
        let Some(pos) = line.find("testsigning") else { continue };
        let rest = &line[pos + "testsigning".len()..];
        if rest.contains("Yes") || rest.contains("Oui") {
            return Some(true);
        }
        if rest.contains("No") || rest.contains("Non") {
            return Some(false);
        }
    }
    None
}

/// Détermine si l'« Intégrité de la mémoire » (HVCI) est active, à partir de
/// la sortie de `reg query ... /v Enabled` (valeur `0x1` / `0x0`).
pub fn parse_hvci_enabled(reg_query_out: &str) -> Option<bool> {
    for line in reg_query_out.lines() {
        if line.contains("Enabled") && line.contains("REG_DWORD") {
            let token = line.split_whitespace().last()?;
            let digits = token.trim_start_matches("0x");
            return digits.parse::<u32>().ok().map(|v| v != 0);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_files_present() {
        assert!(SYS_BYTES.len() > 5_000, "throttle.sys doit être embarqué");
        assert!(CER_BYTES.len() > 500, "le certificat doit être embarqué");
        // Signature Authenticode : le PE commence par "MZ" ;
        // le certificat est en PEM (« -----BEGIN CERTIFICATE----- ») ou DER.
        assert_eq!(&SYS_BYTES[..2], b"MZ");
        let pem = CER_BYTES.starts_with(b"-----");
        let der = CER_BYTES.first() == Some(&0x30);
        assert!(pem || der, "certificat PEM ou DER attendu");
    }

    #[test]
    fn parse_testsigning_francais() {
        let out = "Chemin d'accession du Gestionnaire de demarrage\r\ntestsigning             Oui\r\nnointegritychecks        Non";
        assert_eq!(parse_testsigning(out), Some(true));
    }

    #[test]
    fn parse_testsigning_anglais() {
        let out = "Windows Boot Loader\r\n---------------------\r\ntestsigning             No";
        assert_eq!(parse_testsigning(out), Some(false));
    }

    #[test]
    fn parse_testsigning_absent() {
        assert_eq!(parse_testsigning("identifier {current}"), None);
        assert_eq!(parse_testsigning(""), None);
    }

    #[test]
    fn install_actions_active_testmode() {
        let actions = install_actions(Some(true), Some(false));
        assert!(!actions.contains(&Action::EnableTestsigning));
        assert!(!actions.contains(&Action::DisableHvci));
        assert_eq!(actions.len(), 5);
        assert_eq!(actions[0], Action::InstallCert);
        assert_eq!(*actions.last().unwrap(), Action::ScheduleRelaunch);
    }

    #[test]
    fn install_actions_inactive_testmode() {
        for state in [Some(false), None] {
            let actions = install_actions(state, Some(false));
            assert!(actions.contains(&Action::EnableTestsigning));
            assert_eq!(actions.len(), 6);
        }
    }

    #[test]
    fn install_actions_hvci_active() {
        let actions = install_actions(Some(true), Some(true));
        assert!(actions.contains(&Action::DisableHvci));
        assert_eq!(actions.len(), 6);
        // La désactivation HVCI doit venir juste avant le chargement du filtre.
        let pos = actions.iter().position(|a| *a == Action::DisableHvci).unwrap();
        assert_eq!(actions[pos + 1], Action::LoadFilter);
    }

    #[test]
    fn parse_hvci_detections() {
        let on = "\nHKEY_LOCAL_MACHINE\\SYSTEM\\...\\HypervisorEnforcedCodeIntegrity\n    Enabled    REG_DWORD    0x1\n\n";
        let off = "\n    Enabled    REG_DWORD    0x0\n";
        assert_eq!(parse_hvci_enabled(on), Some(true));
        assert_eq!(parse_hvci_enabled(off), Some(false));
        assert_eq!(parse_hvci_enabled("ERREUR : impossible de trouver"), None);
        assert_eq!(parse_hvci_enabled(""), None);
    }
}
