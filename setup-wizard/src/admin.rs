//! Élévation administrateur : détection, relance via UAC, boîte d'erreur.

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_OK, SW_SHOWNORMAL,
};

/// Le processus dispose-t-il d'un jeton élevé (administrateur) ?
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: isize = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Chemin de l'exécutable courant (UTF-8).
pub fn self_path() -> String {
    unsafe {
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(std::mem::zeroed(), buf.as_mut_ptr(), buf.len() as u32);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

/// Relance l'exécutable courant avec le verbe « runas » (invite UAC).
/// `params` est passé en ligne de commande au nouveau processus (ex. `--auto`).
/// Renvoie false si l'utilisateur refuse ou si l'élévation échoue.
pub fn relaunch_elevated(params: Option<&str>) -> bool {
    let path = self_path();
    let file: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let args: Vec<u16> = params
        .map(|p| p.encode_utf16().chain(std::iter::once(0)).collect())
        .unwrap_or_default();
    unsafe {
        let mut sei: SHELLEXECUTEINFOW = std::mem::zeroed();
        sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        sei.fMask = 0x0000_1000; // SEE_MASK_NOASYNC : on se termine juste après
        sei.lpVerb = verb.as_ptr();
        sei.lpFile = file.as_ptr();
        if !args.is_empty() {
            sei.lpParameters = args.as_ptr();
        }
        sei.nShow = SW_SHOWNORMAL;
        ShellExecuteExW(&mut sei) != 0
    }
}

/// Boîte de message d'erreur (Win32, sans console).
pub fn error_box(title: &str, message: &str) {
    unsafe {
        let t: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let m: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
        MessageBoxW(std::mem::zeroed(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONERROR);
    }
}
