//! Assistant d'installation (setup wizard) du limiteur de débit de dossier.
//!
//! Un seul exécutable autonome : le driver `throttle.sys` et son certificat
//! de test sont embarqués. L'assistant :
//!   1. s'élève lui-même (UAC) ;
//!   2. installe le certificat de test (Racine + Éditeurs approuvés) ;
//!   3. active le mode test-signature si nécessaire ;
//!   4. copie le driver puis crée et charge le service minifilter.
//! Il sait aussi tout désinstaller.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod admin;

use throttle_setup::logic::{self, Action};

#[cfg(feature = "gui")]
use eframe::egui;

// ---------------------------------------------------------------------------
// Point d'entrée
// ---------------------------------------------------------------------------

#[cfg(all(feature = "gui", windows))]
fn main() -> Result<(), eframe::Error> {
    let auto = std::env::args().any(|a| a == "--auto");
    if !admin::is_elevated() {
        // Relance en administrateur : une fenêtre UAC apparaît.
        if admin::relaunch_elevated(if auto { Some("--auto") } else { None }) {
            return Ok(());
        }
        admin::error_box(
            "Droits administrateur requis",
            "Cet assistant doit s'exécuter en tant qu'administrateur.\n\
             Relance-le et accepte la fenêtre UAC.",
        );
        return Ok(());
    }
    if auto {
        // Relance programmée : consomme l'entrée RunOnce pour ne pas boucler.
        let _ = logic::run_cmd(
            "reg",
            &["delete", logic::RUNONCE_KEY, "/v", logic::RUNONCE_VALUE, "/f"],
        );
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Assistant d'installation — Limiteur de débit de dossier")
            .with_inner_size([620.0, 500.0])
            .with_min_inner_size([540.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Assistant Limiteur de débit",
        options,
        Box::new(|_cc| Ok(Box::new(SetupApp::new(auto)))),
    )
}

#[cfg(all(feature = "gui", not(windows)))]
fn main() -> Result<(), eframe::Error> {
    // Aperçu possible sur d'autres plateformes : les commandes système sont stubbées.
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Assistant d'installation — Limiteur de débit de dossier")
            .with_inner_size([620.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Assistant Limiteur de débit",
        options,
        Box::new(|_cc| Ok(Box::new(SetupApp::new(false)))),
    )
}

#[cfg(not(feature = "gui"))]
fn main() {}

// ---------------------------------------------------------------------------
// État de l'assistant
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Welcome,
    Progress,
    Done,
}

#[derive(Clone, Copy, PartialEq)]
enum Intent {
    None,
    Install,
    Uninstall,
}

struct SetupApp {
    page: Page,
    intent: Intent,
    queue: std::collections::VecDeque<Action>,
    log: String,
    started: bool,
    testsigning: Option<bool>,
    hvci: Option<bool>,
    service_installed: bool,
    filter_loaded: bool,
    needs_reboot: bool,
    app_path: Option<String>,
}

impl SetupApp {
    fn new(auto: bool) -> Self {
        let mut log = String::new();
        log.push_str("Assistant d'installation — Limiteur de débit de dossier v1.0\n");
        log.push_str(&format!(
            "Driver et certificat embarqués : {} + {} octets.\n\n",
            logic::SYS_BYTES.len(),
            logic::CER_BYTES.len()
        ));
        let (page, intent, started) = if auto {
            log.push_str("Relance automatique après redémarrage : reprise de l'installation…\n\n");
            (Page::Progress, Intent::Install, true)
        } else {
            (Page::Welcome, Intent::None, false)
        };
        let mut queue = std::collections::VecDeque::new();
        if auto {
            queue.push_back(Action::CheckState);
        }
        Self {
            page,
            intent,
            queue,
            log,
            started,
            testsigning: None,
            hvci: None,
            service_installed: false,
            filter_loaded: false,
            needs_reboot: false,
            app_path: find_app_exe(),
        }
    }

    fn say(&mut self, line: impl Into<String>) {
        self.log.push_str(&line.into());
        self.log.push('\n');
    }

    fn start_install(&mut self) {
        self.queue.clear();
        self.intent = Intent::Install;
        self.filter_loaded = false;
        self.page = Page::Progress;
        self.queue.push_back(Action::CheckState);
    }

    fn start_uninstall(&mut self) {
        self.queue.clear();
        self.intent = Intent::Uninstall;
        self.filter_loaded = false;
        self.page = Page::Progress;
        self.queue.push_back(Action::UnloadFilter);
        self.queue.push_back(Action::DeleteService);
        self.queue.push_back(Action::RemoveFiles);
    }

    fn reboot_now(&mut self) {
        self.say("[info] redémarrage dans 5 secondes — pense à sauvegarder ton travail !");
        #[cfg(windows)]
        let (ok, out) = logic::run_cmd("shutdown", &["/r", "/t", "5"]);
        #[cfg(not(windows))]
        let (ok, out) = (false, String::from("[hors Windows : commande non executee]"));
        if !ok {
            self.say(format!("[erreur] shutdown : {}", short(&out)));
        }
    }

    // ---- Exécution d'une action (une par frame, pour un journal vivant) ----

    fn run_action(&mut self, action: Action) {
        match action {
            Action::CheckState => {
                let (_ok, out) = logic::run_cmd("bcdedit", &["/enum", "{current}"]);
                self.testsigning = logic::parse_testsigning(&out);
                match self.testsigning {
                    Some(true) => self.say("[etat] mode test-signature : ACTIF"),
                    Some(false) => self.say("[etat] mode test-signature : INACTIF"),
                    None => self.say("[etat] mode test-signature : inconnu"),
                }
                // Intégrité de la mémoire (HVCI) : clé moderne puis ancienne.
                let (okh, outh) = logic::run_cmd("reg", &["query", logic::HVCI_KEY, "/v", "Enabled"]);
                self.hvci = if okh {
                    logic::parse_hvci_enabled(&outh)
                } else {
                    let (_ok2, out2) =
                        logic::run_cmd("reg", &["query", logic::HVCI_KEY_OLD, "/v", "EnableHVCI"]);
                    logic::parse_hvci_enabled(&out2)
                };
                match self.hvci {
                    Some(true) => self.say("[etat] intégrité de la mémoire (HVCI) : ACTIVE — Windows 11 bloque les drivers de test tant qu'elle est active"),
                    Some(false) => self.say("[etat] intégrité de la mémoire (HVCI) : inactive"),
                    // Clé absente = HVCI non forcée (valeur par défaut hors
                    // stratégies d'entreprise) — c'est ce qu'on veut.
                    None => self.say("[etat] intégrité de la mémoire (HVCI) : désactivée (clé absente)"),
                }
                let (ok, _) = logic::run_cmd("sc", &["query", logic::SVC]);
                self.service_installed = ok;
                self.say(&format!(
                    "[etat] service '{}' : {}",
                    logic::SVC,
                    if ok { "installé" } else { "absent" }
                ));
                if self.intent == Intent::Install {
                    for a in logic::install_actions(self.testsigning, self.hvci) {
                        self.queue.push_back(a);
                    }
                }
            }
            Action::InstallCert => {
                if let Err(e) = std::fs::create_dir_all(logic::INSTALL_DIR) {
                    self.say(format!(
                        "[erreur] création de {} impossible : {e}",
                        logic::INSTALL_DIR
                    ));
                    return;
                }
                if let Err(e) = std::fs::write(logic::CER_PATH, logic::CER_BYTES) {
                    self.say(format!("[erreur] écriture du certificat : {e}"));
                    return;
                }
                let (ok1, out1) = logic::run_cmd("certutil", &["-addstore", "Root", logic::CER_PATH]);
                self.say(if ok1 {
                    "[ok] certificat de test installé dans la boutique « Racine »".to_string()
                } else {
                    format!("[erreur] certutil (Racine) : {}", short(&out1))
                });
                let (ok2, out2) = logic::run_cmd(
                    "certutil",
                    &["-addstore", "-f", "TrustedPublisher", logic::CER_PATH],
                );
                self.say(if ok2 {
                    "[ok] certificat ajouté aux « Éditeurs approuvés »".to_string()
                } else {
                    format!("[erreur] certutil (Éditeurs approuvés) : {}", short(&out2))
                });
            }
            Action::EnableTestsigning => {
                let (ok, out) = logic::run_cmd("bcdedit", &["/set", "testsigning", "on"]);
                if ok {
                    self.needs_reboot = true;
                    self.say("[ok] mode test-signature activé (bcdedit) — un redémarrage est requis");
                } else {
                    self.say(format!("[erreur] bcdedit : {}", short(&out)));
                }
            }
            Action::DeploySys => {
                if let Err(e) = std::fs::create_dir_all(logic::INSTALL_DIR) {
                    self.say(format!("[erreur] création du dossier : {e}"));
                    return;
                }
                match std::fs::write(logic::SYS_PATH, logic::SYS_BYTES) {
                    Ok(()) => self.say(format!(
                        "[ok] driver copié : {} ({} octets)",
                        logic::SYS_PATH,
                        logic::SYS_BYTES.len()
                    )),
                    Err(e) => self.say(format!("[erreur] écriture du driver : {e}")),
                }
            }
            Action::CreateService => {
                let (ok, out) = logic::run_cmd(
                    "sc",
                    &[
                        "create",
                        logic::SVC,
                        "type=",
                        "filesys",
                        "start=",
                        "demand",
                        "binPath=",
                        logic::SYS_PATH,
                        "DisplayName=",
                        logic::DISPLAY_NAME,
                    ],
                );
                self.say(if ok {
                    format!("[ok] service '{}' créé", logic::SVC)
                } else {
                    format!(
                        "[info] service '{}' déjà présent ou échec (on continue) : {}",
                        logic::SVC,
                        short(&out)
                    )
                });
                let _ = logic::run_cmd(
                    "sc",
                    &["description", logic::SVC, logic::DESCRIPTION],
                );
                let base = format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{}", logic::SVC);
                let (ok, out) = logic::run_cmd(
                    "reg",
                    &["add", &base, "/v", "DefaultInstance", "/t", "REG_SZ", "/d", logic::INSTANCE_KEY, "/f"],
                );
                log_reg(self, ok, &out, "instance par défaut");
                let inst_key = format!(r"{base}\Instances\{}", logic::INSTANCE_KEY);
                let (ok, out) = logic::run_cmd(
                    "reg",
                    &["add", &inst_key, "/v", "Altitude", "/t", "REG_SZ", "/d", "399999", "/f"],
                );
                log_reg(self, ok, &out, "altitude");
                let (ok, out) = logic::run_cmd(
                    "reg",
                    &["add", &inst_key, "/v", "Flags", "/t", "REG_DWORD", "/d", "0", "/f"],
                );
                log_reg(self, ok, &out, "flags");
            }
            Action::LoadFilter => {
                let (ok, out) = logic::run_cmd("fltmc", &["load", logic::SVC]);
                self.filter_loaded = ok;
                if ok {
                    self.say(format!(
                        "[ok] filtre chargé — installation opérationnelle\n\nTermine : lance throttle-folder.exe (il est déjà en administrateur)."
                    ));
                } else {
                    self.say(format!("[erreur] fltmc load : {}", short(&out)));
                    if self.needs_reboot {
                        self.say("→ Normal : le mode test et/ou l'HVCI viennent d'être modifiés.\n→ REDÉMARRE le PC, puis relance cet assistant : il chargera le filtre et terminera l'installation.");
                    } else {
                        self.say("→ Vérifie : session administrateur, mode test actif (bcdedit /enum {current}), et « Intégrité de la mémoire » désactivée (Sécurité Windows → Isolation du noyau).\n→ Code 0xC0000428 = signature refusée. Code 0x800701E7 = image du driver refusée par le noyau (structure/alignement) — installe la dernière version de l'assistant.");
                    }
                }
            }
            Action::DisableHvci => {
                // Windows 11 : HVCI active = aucun driver test-signé ne se charge,
                // même avec le mode test. On coupe la clé moderne + l'ancienne.
                let (ok, out) = logic::run_cmd(
                    "reg",
                    &["add", logic::HVCI_KEY, "/v", "Enabled", "/t", "REG_DWORD", "/d", "0", "/f"],
                );
                let _ = logic::run_cmd(
                    "reg",
                    &["add", logic::HVCI_KEY_OLD, "/v", "EnableHVCI", "/t", "REG_DWORD", "/d", "0", "/f"],
                );
                if ok {
                    self.needs_reboot = true;
                    self.say("[ok] intégrité de la mémoire (HVCI) désactivée — un redémarrage est requis");
                    self.say("→ Indispensable pour charger un driver de test sur Windows 11. Tu pourras la réactiver plus tard dans Sécurité Windows → Isolation du noyau (mais le driver ne se chargera plus tant qu'elle est active).");
                } else {
                    self.say(format!("[erreur] désactivation HVCI : {}", short(&out)));
                }
            }
            Action::ScheduleRelaunch => {
                if !self.needs_reboot {
                    self.say("[info] aucun redémarrage nécessaire : relance automatique non requise.");
                } else {
                    #[cfg(windows)]
                    {
                        let exe = admin::self_path();
                        let val = format!("\"{}\" --auto", exe);
                        let (ok, out) = logic::run_cmd(
                            "reg",
                            &[
                                "add",
                                logic::RUNONCE_KEY,
                                "/v",
                                logic::RUNONCE_VALUE,
                                "/t",
                                "REG_SZ",
                                "/d",
                                &val,
                                "/f",
                            ],
                        );
                        if ok {
                            self.say("[ok] l'assistant se relancera TOUT SEUL après le redémarrage pour terminer l'installation — tu n'auras qu'à accepter la fenêtre UAC.");
                        } else {
                            self.say(format!("[info] relance automatique non programmée ({}) : relance simplement l'assistant après le redémarrage.", short(&out)));
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        self.say("[info] relance automatique : hors Windows, ignoré.");
                    }
                }
            }
            Action::UnloadFilter => {
                let (ok, out) = logic::run_cmd("fltmc", &["unload", logic::SVC]);
                self.say(if ok {
                    format!("[ok] filtre '{}' déchargé", logic::SVC)
                } else {
                    format!("[info] filtre non chargé (rien à décharger) : {}", short(&out))
                });
            }
            Action::DeleteService => {
                let (ok, out) = logic::run_cmd("sc", &["delete", logic::SVC]);
                self.say(if ok {
                    format!("[ok] service '{}' supprimé", logic::SVC)
                } else {
                    format!("[info] service absent ou suppression impossible : {}", short(&out))
                });
                let base = format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{}", logic::SVC);
                let _ = logic::run_cmd("reg", &["delete", &base, "/f"]);
            }
            Action::RemoveFiles => {
                for path in [logic::SYS_PATH, logic::CER_PATH] {
                    match std::fs::remove_file(path) {
                        Ok(()) => self.say(format!("[ok] supprimé : {path}")),
                        Err(_) => self.say(format!("[info] absent : {path}")),
                    }
                }
            }
        }
    }

    fn finish(&mut self) {
        self.page = Page::Done;
    }
}

fn log_reg(app: &mut SetupApp, ok: bool, out: &str, quoi: &str) {
    app.say(if ok {
        format!("[ok] registre : {quoi}")
    } else {
        format!("[erreur] registre ({quoi}) : {}", short(out))
    });
}

/// Première ligne significative d'une sortie de commande (pour le journal).
fn short(out: &str) -> String {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .next()
        .unwrap_or("(pas de message)")
        .chars()
        .take(160)
        .collect()
}

#[cfg(windows)]
fn find_app_exe() -> Option<String> {
    let me = admin::self_path();
    let dir = std::path::Path::new(&me).parent()?;
    let cand = dir.join("throttle-folder.exe");
    cand.exists().then(|| cand.to_string_lossy().into_owned())
}

#[cfg(not(windows))]
fn find_app_exe() -> Option<String> {
    None
}

#[cfg(windows)]
fn launch_app(path: &str) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(path)
        .creation_flags(0x0800_0000)
        .spawn()
        .is_ok()
}

#[cfg(not(windows))]
fn launch_app(_path: &str) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Interface (egui) — sobre : fond noir, texte blanc, boutons
// ---------------------------------------------------------------------------

#[cfg(feature = "gui")]
impl eframe::App for SetupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // État initial : une détection au premier passage.
        if !self.started {
            self.started = true;
            self.queue.push_back(Action::CheckState);
        }

        // Au plus UNE action par frame → le journal s'anime.
        if let Some(action) = self.queue.pop_front() {
            self.run_action(action);
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        } else if self.page == Page::Progress {
            self.finish();
        }

        ctx.style_mut(|style| {
            let v = &mut style.visuals;
            v.panel_fill = egui::Color32::BLACK;
            v.window_fill = egui::Color32::BLACK;
            v.extreme_bg_color = egui::Color32::BLACK;
            v.faint_bg_color = egui::Color32::from_gray(18);
        });

        egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                match self.page {
                    Page::Welcome => {
                        if bouton(ui, "Installer") {
                            self.start_install();
                        }
                        if bouton(ui, "Désinstaller") {
                            self.start_uninstall();
                        }
                        if bouton(ui, "Quitter") {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                    Page::Progress => {
                        ui.add(egui::Label::new(
                            egui::RichText::new("Traitement en cours...").weak(),
                        ));
                        ui.add_space(8.0);
                    }
                    Page::Done => {
                        if bouton(ui, "Fermer") {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
            });
            ui.add_space(10.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Welcome => self.draw_welcome(ui),
            Page::Progress | Page::Done => self.draw_log(ui),
        });
    }
}

#[cfg(feature = "gui")]
fn bouton(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add_sized([120.0, 30.0], egui::Button::new(label)).clicked()
}

#[cfg(feature = "gui")]
impl SetupApp {
    fn draw_welcome(&mut self, ui: &mut egui::Ui) {
        ui.heading("Limiteur de débit de dossier — Installation");
        ui.add_space(8.0);
        ui.label("Cet assistant installe le driver minifilter qui limite la vitesse de lecture/écriture d'un dossier, pour tous les processus.");
        ui.add_space(10.0);
        ui.label("Il va :");
        ui.label("  1. Installer le certificat de test (Racine + Éditeurs approuvés) ;");
        ui.label("  2. Activer le mode test-signature si besoin, et désactiver l'« Intégrité");
        ui.label("      de la mémoire » de Windows 11 si elle est active (elle bloque les");
        ui.label("      drivers de test, même en mode test) — un redémarrage sera demandé ;");
        ui.label(format!("  3. Copier le driver dans {}", logic::INSTALL_DIR));
        ui.label("  4. Créer le service minifilter puis charger le filtre.");
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(
                "Driver et certificat sont embarqués dans cet assistant : rien d'autre à télécharger.",
            )
            .weak(),
        );
        ui.label(
            egui::RichText::new(
                "Après un éventuel redémarrage, l'assistant se relance tout seul et termine l'installation.",
            )
            .weak(),
        );
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(format!(
            "Mode test-signature : {}",
            match self.testsigning {
                Some(true) => "actif".to_string(),
                Some(false) => "inactif (sera activé)".to_string(),
                None => "détection...".to_string(),
            }
        ));
        ui.label(format!(
            "Intégrité de la mémoire (HVCI) : {}",
            match self.hvci {
                Some(true) => "ACTIVE — sera désactivée (nécessaire pour un driver de test)".to_string(),
                Some(false) => "inactive (parfait)".to_string(),
                None => "détection...".to_string(),
            }
        ));
        ui.label(format!(
            "Service du driver : {}",
            if self.service_installed { "installé" } else { "non installé" }
        ));
        if self.app_path.is_some() {
            ui.label("Application : trouvée à côté de l'assistant (elle sera lançable à la fin).");
        }
    }

    fn draw_log(&mut self, ui: &mut egui::Ui) {
        if self.page == Page::Done {
            self.draw_done_summary(ui);
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
        }
        egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
            for line in self.log.lines() {
                let tone = if line.starts_with("[erreur]") {
                    egui::Color32::from_rgb(235, 100, 100)
                } else if line.starts_with("[ok]") {
                    egui::Color32::from_rgb(140, 220, 140)
                } else if line.starts_with("[info]") || line.starts_with("[etat]") {
                    egui::Color32::from_gray(160)
                } else {
                    egui::Color32::WHITE
                };
                ui.add(
                    egui::Label::new(egui::RichText::new(line).monospace().color(tone))
                        .wrap(),
                );
            }
            if self.page == Page::Progress {
                ui.add_space(2.0);
                ui.spinner();
            }
        });
    }

    fn draw_done_summary(&mut self, ui: &mut egui::Ui) {
        match self.intent {
            Intent::Install => {
                if self.filter_loaded {
                    ui.heading("Installation terminée");
                    ui.label("Le limiteur est actif : lance throttle-folder.exe pour choisir le dossier et la vitesse.");
                    if let Some(path) = self.app_path.clone() {
                        ui.add_space(6.0);
                        if bouton(ui, "Lancer l'application") {
                            let _ = launch_app(&path);
                        }
                    }
                } else if self.needs_reboot {
                    ui.heading("Presque terminé — redémarrage requis");
                    ui.label("Le mode test et/ou l'« Intégrité de la mémoire » ont été modifiés : un redémarrage est nécessaire.");
                    ui.label("Après le redémarrage, l'assistant SE RELANCERA TOUT SEUL et terminera l'installation (tu n'auras qu'à accepter l'UAC).");
                    ui.add_space(6.0);
                    if bouton(ui, "Redémarrer maintenant (5 s)") {
                        self.reboot_now();
                    }
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Tu peux aussi redémarrer toi-même plus tard : relance simplement cet assistant après.")
                            .weak(),
                    );
                } else {
                    ui.heading("L'installation n'a pas abouti");
                    ui.label("Consulte le journal ci-dessous : la cause y est indiquée (droits, certificat, mode test, HVCI).");
                }
            }
            Intent::Uninstall => {
                ui.heading("Désinstallation terminée");
                ui.label("Le filtre est déchargé et le service supprimé.");
                ui.label(
                    egui::RichText::new(
                        "Pour quitter le mode test-signature : bcdedit /set testsigning off, puis redémarre.",
                    )
                    .weak(),
                );
            }
            Intent::None => {
                ui.heading("Détection terminée");
            }
        }
    }
}
