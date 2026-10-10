//! Logique de l'assistant : actions à exécuter, exécution des commandes
//! système, détection de l'état. Testable hors Windows (commandes stubbées).

// ---- Fichiers embarqués dans l'exécutable --------------------------------

pub const SYS_BYTES: &[u8] = include_bytes!("../embed/throttle.sys");
pub const CER_BYTES: &[u8] = include_bytes!("../embed/throttle-test.cer");

// ---- Constantes d'installation -------------------------------------------

pub const SVC: &str = "throttle";
pub const INSTALL_DIR: &str = r"C:\ProgramData\ThrottleFolder";
pub const SYS_PATH: &str = r"C:\ProgramData\ThrottleFolder\throttle.sys";
/// ImagePath au format NT — LA forme que le chargeur noyau exige dans la
/// valeur ImagePath du service. Un chemin Win32 nu (« C:\... ») est préfixé
/// en interne par `\SystemRoot\` et devient introuvable : fltmc load
/// répond alors 0x80070002 (fichier introuvable) alors que le fichier existe.
/// C'est exactement le bug observé sur le terrain : le service pré-existant
/// pointait vers `\??\C:\ProgramData\...` et le `sc config` du wizard
/// l'écrasait avec la forme Win32 — cassant un ImagePath correct.
pub const SYS_NT_PATH: &str = r"\??\C:\ProgramData\ThrottleFolder\throttle.sys";
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

/// Extrait le champ BINARY_PATH_NAME d'une sortie `sc qc <service>`.
/// Format typique (la locale change le libellé autour, pas la valeur) :
/// « BINARY_PATH_NAME   : \??\C:\ProgramData\ThrottleFolder\throttle.sys ».
/// On coupe au PREMIER « : » uniquement — la valeur (chemin DOS) contient
/// elle-même des « : » qu'il ne faut pas interpréter.
pub fn parse_binary_path(sc_qc_out: &str) -> Option<String> {
    for line in sc_qc_out.lines() {
        let t = line.trim();
        if t.contains("BINARY_PATH") || t.contains("binPath") {
            if let Some((_avant, apres)) = t.split_once(':') {
                let p = apres.trim();
                if !p.is_empty() {
                    return Some(p.to_string());
                }
            }
        }
    }
    None
}

/// Un ImagePath exploitable par le chargeur noyau doit être un chemin NT :
/// `\??\...` (chemin DOS préfixé) ou `\SystemRoot\...` (relatif à Windows).
/// Un chemin Win32 nu (« C:\... ») n'en fait PAS partie.
pub fn nt_path_ok(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.starts_with(r"\??\") || p.starts_with(r"\systemroot\")
}

/// Détermine si le filtre « throttle » est déjà chargé, à partir de la sortie
/// de `fltmc filters`. Les en-têtes sont localisés mais la ligne de données
/// contient le nom du service littéral dans sa première colonne.
pub fn parse_filter_loaded(fltmc_out: &str) -> bool {
    fltmc_out.lines().any(|l| {
        l.split_whitespace()
            .any(|w| w.eq_ignore_ascii_case("throttle"))
    })
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

    #[test]
    fn chemin_nt_conforme() {
        assert!(nt_path_ok(r"\??\C:\ProgramData\ThrottleFolder\throttle.sys"));
        assert!(nt_path_ok(r"\SystemRoot\System32\drivers\throttle.sys"));
        // Les formes cassées observées sur le terrain :
        assert!(!nt_path_ok(r"C:\ProgramData\ThrottleFolder\throttle.sys"));
        assert!(!nt_path_ok("throttle.sys"));
        // La constante embarquée DOIT rester la forme NT canonique : si un
        // jour on la « simplifie » en chemin Win32, ce test doit saigner.
        assert!(nt_path_ok(SYS_NT_PATH), "SYS_NT_PATH doit être un chemin NT");
        assert!(SYS_NT_PATH.ends_with(r"\throttle.sys"));
        assert_eq!(SYS_PATH, &SYS_NT_PATH[4..]);
    }

    #[test]
    fn parse_sc_qc_binary_path() {
        // Sortie réelle de sc qc (locale FR : le libellé reste anglais).
        let out = "\r\n        TYPE               : 2   FILE_SYSTEM_DRIVER\r\n\
                  \x20        BINARY_PATH_NAME   : \\??\\C:\\ProgramData\\ThrottleFolder\\throttle.sys\r\n\
                  \x20        LOAD_ORDER_GROUP   : FSFilter Activity Monitor\r\n";
        assert_eq!(
            parse_binary_path(out).as_deref(),
            Some(r"\??\C:\ProgramData\ThrottleFolder\throttle.sys")
        );
        // Les « : » du chemin DOS ne doivent pas couper l'extraction.
        assert_eq!(
            parse_binary_path("BINARY_PATH_NAME : \\??\\C:\\a b\\c.sys\n").as_deref(),
            Some(r"\??\C:\a b\c.sys")
        );
        // Sortie sans champ binaire → None (et pas de panique).
        assert_eq!(parse_binary_path("  [SC] QueryServiceConfig : SUCCESS\n"), None);
        assert_eq!(parse_binary_path(""), None);
    }

    #[test]
    fn parse_filtre_deja_charge() {
        let charge = "\nNombre de filtres : 3\n\n\nHauteur du filtre   Nom du filtre   Instances   Cadre\n\n     399999    throttle     1    0\n     389999    luafv        1    0\n";
        let absent = "\nNombre de filtres : 2\n\n     389999    luafv        1    0\n     340000    wcifs        1    0\n";
        // La ligne d'en-tête localisée ne doit PAS faire croire au chargement.
        let entete_seule = "\nNom du filtre     Altitude\n\n";
        assert!(parse_filter_loaded(charge));
        assert!(!parse_filter_loaded(absent));
        assert!(!parse_filter_loaded(entete_seule));
        assert!(!parse_filter_loaded(""));
    }
}
