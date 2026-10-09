//! Communication avec le minifilter « throttle » via le port `\ThrottlePort`.
//!
//! Sous Windows : `fltlib.dll` (FilterConnectCommunicationPort / FilterSendMessage).
//! Ailleurs : stubs qui renvoient une erreur — l'interface propose alors le
//! mode simulation. Les déclarations windows-sys ont été vérifiées dans la
//! doc/les sources de la crate : les fonctions vivent dans
//! `Win32::Storage::InstallableFileSystems` (feature `Win32_Storage_InstallableFileSystems`,
//! liaison `fltlib.dll`).

use crate::protocol::{ThrottleMessage, ThrottleStats};

#[cfg(windows)]
use crate::protocol::CMD_GET_STATS;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::Storage::InstallableFileSystems::{
    FilterConnectCommunicationPort, FilterSendMessage,
};

/// Nom du port de communication créé par le driver (identique côté C :
/// `L"\\ThrottlePort"`).
pub const PORT_NAME: &str = "\\ThrottlePort";

/// Connexion ouverte vers le minifilter. Fermée automatiquement (Drop).
pub struct DriverLink {
    #[cfg(windows)]
    handle: HANDLE,
    #[cfg(not(windows))]
    _priv: (),
}

impl DriverLink {
    /// Se connecte au port du driver. Nécessite un processus administrateur
    /// et le driver chargé (`fltmc load throttle`).
    pub fn connect() -> Result<DriverLink, String> {
        #[cfg(windows)]
        {
            let wide: Vec<u16> = PORT_NAME
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            // NB : dans windows-sys 0.52, HANDLE est un isize.
            let mut handle: HANDLE = 0;
            let hr = unsafe {
                FilterConnectCommunicationPort(
                    wide.as_ptr(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    &mut handle,
                )
            };
            if hr < 0 {
                let hint = if (hr as u32) & 0xFFFF == 5 {
                    " — lance l'application en tant qu'administrateur"
                } else {
                    " — le driver « throttle » est-il chargé ? (fltmc load throttle)"
                };
                return Err(format!(
                    "Connexion au driver impossible (HRESULT 0x{:08X}){}",
                    hr as u32, hint
                ));
            }
            Ok(DriverLink { handle })
        }
        #[cfg(not(windows))]
        Err("Le driver n'est disponible que sous Windows.".to_string())
    }

    /// Envoie un message au driver (SET_CONFIG ou CLEAR).
    pub fn send_message(&self, msg: &ThrottleMessage) -> Result<(), String> {
        #[cfg(windows)]
        unsafe {
            let mut returned: u32 = 0;
            let hr = FilterSendMessage(
                self.handle,
                msg as *const ThrottleMessage as *const core::ffi::c_void,
                std::mem::size_of::<ThrottleMessage>() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
            );
            if hr < 0 {
                return Err(format!(
                    "Envoi au driver impossible (HRESULT 0x{:08X})",
                    hr as u32
                ));
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = msg;
            Err("Le driver n'est disponible que sous Windows.".to_string())
        }
    }

    /// Interroge le driver : compteurs cumulés et débits mesurés.
    pub fn get_stats(&self) -> Result<ThrottleStats, String> {
        #[cfg(windows)]
        unsafe {
            let req = ThrottleMessage {
                command: CMD_GET_STATS,
                _pad0: 0,
                config: Default::default(),
            };
            let mut stats = ThrottleStats::default();
            let mut returned: u32 = 0;
            let hr = FilterSendMessage(
                self.handle,
                &req as *const ThrottleMessage as *const core::ffi::c_void,
                std::mem::size_of::<ThrottleMessage>() as u32,
                &mut stats as *mut ThrottleStats as *mut core::ffi::c_void,
                std::mem::size_of::<ThrottleStats>() as u32,
                &mut returned,
            );
            if hr < 0 {
                return Err(format!(
                    "Statistiques indisponibles (HRESULT 0x{:08X})",
                    hr as u32
                ));
            }
            Ok(stats)
        }
        #[cfg(not(windows))]
        Err("Le driver n'est disponible que sous Windows.".to_string())
    }
}

impl Drop for DriverLink {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

/// Convertit un chemin DOS (« C:\dossier ») en chemin device normalisé
/// (« \Device\HarddiskVolume3\dossier »).
///
/// Le driver compare ce chemin au préfixe des noms de fichiers NORMALISÉS
/// renvoyés par FltGetFileNameInformation — lesquels commencent toujours par
/// la forme device du volume, jamais par la lettre de lecteur. `QueryDosDeviceW`
/// fait la traduction côté application, une seule fois, avant l'envoi.
///
/// Chemins UNC / non locaux : renvoyés tels quels (non pris en charge en v1).
pub fn to_device_path(path: &str) -> String {
    #[cfg(windows)]
    {
        let bytes = path.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            let drive = format!("{}:", (bytes[0] as char).to_ascii_uppercase());
            let drive_wide: Vec<u16> = drive
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let mut buf = [0u16; 1024];
            let n = unsafe {
                windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW(
                    drive_wide.as_ptr(),
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                )
            };
            if n > 0 {
                let dev_end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                if let Ok(dev) = String::from_utf16(&buf[..dev_end]) {
                    let rest = path[2..].trim_start_matches('\\');
                    return if rest.is_empty() {
                        dev
                    } else {
                        format!("{}\\{}", dev, rest)
                    };
                }
            }
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chemin_non_dos_renvoye_tel_quel() {
        assert_eq!(to_device_path(r"\\serveur\partage\dir"), r"\\serveur\partage\dir");
        assert_eq!(to_device_path("relative/dir"), "relative/dir");
    }
}
