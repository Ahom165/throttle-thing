//! Interface graphique sobre (egui) : fond noir ou blanc, texte, champs,
//! boutons. Rien de plus.

use eframe::egui;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::driver_comm::DriverLink;
use crate::protocol::{CMD_CLEAR, CMD_SET_CONFIG, ThrottleConfig, ThrottleMessage, ThrottleStats};
use crate::sim::{SimCounters, SimHandle};
use crate::units::{format_bytes, format_speed, to_bytes_per_sec, Unit};

/// Durée entre deux rafraîchissements des statistiques.
const REFRESH: Duration = Duration::from_millis(400);
/// Nombre maximum de lignes conservées dans le journal.
const LOG_MAX: usize = 200;

pub struct ThrottleApp {
    /// Thème actuel (noir par défaut).
    theme_dark: bool,
    /// Chemin DOS du dossier à limiter, saisi par l'utilisateur.
    path: String,
    // Limites (valeur saisie + unité), « 0 ou vide = illimité ».
    read_val: String,
    read_unit: Unit,
    write_val: String,
    write_unit: Unit,
    /// Mode simulation (aucune I/O réelle, aucun driver requis).
    use_sim: bool,
    /// État : limitation réelle / simulation en cours.
    active: bool,
    active_sim: bool,
    /// Chemin affiché dans le statut quand actif.
    active_path: String,
    /// Limites appliquées (octets/s) — pour affichage.
    cfg_read: u64,
    cfg_write: u64,
    /// Connexion au driver (None = non connecté).
    link: Option<DriverLink>,
    /// Dernières statistiques (driver ou simulation).
    stats: ThrottleStats,
    last_refresh: Instant,
    /// Dernière erreur à afficher sous les boutons.
    last_error: Option<String>,
    /// Journal d'activité.
    log: Vec<String>,
    /// Simulation en cours, le cas échéant.
    sim: Option<SimHandle>,
    sim_counters: SimCounters,
    /// Instantané précédent pour calculer les débits simulés.
    sim_prev: (u64, u64, Instant),
}

impl ThrottleApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_visuals(&cc.egui_ctx, true);
        let mut app = ThrottleApp {
            theme_dark: true,
            path: String::new(),
            read_val: "0".to_string(),
            read_unit: Unit::Mo,
            write_val: "0".to_string(),
            write_unit: Unit::Mo,
            use_sim: false,
            active: false,
            active_sim: false,
            active_path: String::new(),
            cfg_read: 0,
            cfg_write: 0,
            link: None,
            stats: ThrottleStats::default(),
            last_refresh: Instant::now(),
            last_error: None,
            log: Vec::new(),
            sim: None,
            sim_counters: SimCounters::default(),
            sim_prev: (0, 0, Instant::now()),
        };
        app.push_log("Prêt. Charge le driver (fltmc load throttle) puis indique un dossier.".into());
        app
    }

    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------

    fn start(&mut self) {
        self.last_error = None;

        // 1. Validation du dossier
        let path = self.path.trim().to_string();
        if path.is_empty() {
            self.last_error = Some("Indique d'abord un dossier.".into());
            return;
        }
        if !Path::new(&path).is_dir() {
            self.last_error =
                Some(format!("Dossier introuvable ou inaccessible : « {} »", path));
            return;
        }

        // 2. Validation des limites (0 ou vide = illimité)
        let (read_v, write_v) = match (parse_fr(&self.read_val), parse_fr(&self.write_val)) {
            (Some(r), Some(w)) => (r, w),
            _ => {
                self.last_error = Some("Limites invalides : nombre attendu (ex. 10,5).".into());
                return;
            }
        };
        let read_bps = to_bytes_per_sec(read_v, self.read_unit);
        let write_bps = to_bytes_per_sec(write_v, self.write_unit);

        // 3. Mode simulation : même maths que le driver, zéro impact réel
        if self.use_sim {
            self.stop_sim();
            let counters = SimCounters::default();
            self.sim = Some(SimHandle::start(read_bps, write_bps, counters.clone()));
            self.sim_counters = counters;
            self.sim_prev = (0, 0, Instant::now());
            self.active = true;
            self.active_sim = true;
            self.active_path = path.clone();
            self.cfg_read = read_bps;
            self.cfg_write = write_bps;
            self.stats = ThrottleStats::default();
            self.push_log(format!(
                "Simulation démarrée sur « {} » — lecture : {}, écriture : {}",
                path,
                fmt_limit(read_bps),
                fmt_limit(write_bps)
            ));
            return;
        }

        // 4. Connexion au driver
        if self.link.is_none() {
            match DriverLink::connect() {
                Ok(link) => {
                    self.link = Some(link);
                    self.push_log("Connecté au driver « throttle ».".into());
                }
                Err(e) => {
                    self.last_error = Some(e);
                    self.push_log("Connexion au driver échouée.".into());
                    return;
                }
            }
        }

        // 5. Configuration : chemin en forme device (préfixe des noms
        //    normalisés côté noyau), PID de l'app exclu, débits.
        let mut cfg = ThrottleConfig::default();
        cfg.enabled = 1;
        cfg.read_bps = read_bps;
        cfg.write_bps = write_bps;
        cfg.app_pid = std::process::id() as u64;
        cfg.set_path(&crate::driver_comm::to_device_path(&path));

        let msg = ThrottleMessage {
            command: CMD_SET_CONFIG,
            _pad0: 0,
            config: cfg,
        };
        match self.link.as_ref().unwrap().send_message(&msg) {
            Ok(()) => {
                self.active = true;
                self.active_sim = false;
                self.active_path = path;
                self.cfg_read = read_bps;
                self.cfg_write = write_bps;
                self.stats = ThrottleStats::default();
                self.last_refresh = Instant::now();
                self.push_log(format!(
                    "Limitation appliquée sur « {} » — lecture : {}, écriture : {}",
                    self.active_path,
                    fmt_limit(read_bps),
                    fmt_limit(write_bps)
                ));
            }
            Err(e) => {
                self.last_error = Some(format!("Échec de la configuration : {}", e));
            }
        }
    }

    /// « Annuler » : retire immédiatement toute limitation côté driver
    /// (ou arrête la simulation), sans décharger le filtre.
    fn cancel(&mut self) {
        self.last_error = None;
        if self.active_sim {
            self.stop_sim();
            self.active_sim = false;
        }
        if let Some(link) = &self.link {
            let msg = ThrottleMessage {
                command: CMD_CLEAR,
                _pad0: 0,
                config: ThrottleConfig::default(),
            };
            if let Err(e) = link.send_message(&msg) {
                self.last_error = Some(format!("Annulation impossible : {}", e));
                return;
            }
        }
        self.active = false;
        self.stats = ThrottleStats::default();
        self.push_log("Limitation annulée — tous les débits sont libérés.".into());
    }

    fn stop_sim(&mut self) {
        if let Some(sim) = self.sim.take() {
            sim.stop();
        }
    }

    /// Interroge le driver (ou lit la simulation) et met à jour l'affichage.
    fn refresh_stats(&mut self) {
        self.last_refresh = Instant::now();
        if self.active_sim {
            let tr = self.sim_counters.total_read.load(Ordering::Relaxed);
            let tw = self.sim_counters.total_write.load(Ordering::Relaxed);
            let dt = self.sim_prev.2.elapsed().as_secs_f64().max(1e-3);
            let rb = ((tr.saturating_sub(self.sim_prev.0)) as f64 / dt) as u64;
            let wb = ((tw.saturating_sub(self.sim_prev.1)) as f64 / dt) as u64;
            self.sim_prev = (tr, tw, Instant::now());
            self.stats = ThrottleStats {
                total_read: tr,
                total_write: tw,
                read_bps: rb,
                write_bps: wb,
                active: 1,
                _pad0: 0,
            };
        } else if let Some(link) = &self.link {
            match link.get_stats() {
                Ok(s) => {
                    self.stats = s;
                    self.last_error = None;
                }
                Err(e) => self.last_error = Some(e),
            }
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push(format!("{} {}", hms_now(), line));
        if self.log.len() > LOG_MAX {
            let excess = self.log.len() - LOG_MAX;
            self.log.drain(..excess);
        }
    }

    // ------------------------------------------------------------------
    // Rendu
    // ------------------------------------------------------------------

    fn status_block(ui: &mut egui::Ui, app: &ThrottleApp) {
        if app.active {
            let sim = if app.active_sim { " (simulation)" } else { "" };
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(120, 200, 120), "●");
                ui.label(format!(
                    "Limitation ACTIVE{} — {}",
                    sim,
                    if app.active_path.is_empty() { "…" } else { &app.active_path }
                ));
            });
            let driver_active = app.stats.active == 1;
            let state = if driver_active || app.active_sim {
                "le filtre bridge chaque accès au dossier"
            } else {
                "⚠ le driver n'est plus actif (déchargé ?)"
            };
            ui.small(state);
            ui.add_space(4.0);
            egui::Grid::new("stats").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
                ui.label("Lecture :");
                ui.label(format!(
                    "{} (total {})",
                    format_speed(app.stats.read_bps as f64),
                    format_bytes(app.stats.total_read as f64)
                ));
                ui.end_row();
                ui.label("Écriture :");
                ui.label(format!(
                    "{} (total {})",
                    format_speed(app.stats.write_bps as f64),
                    format_bytes(app.stats.total_write as f64)
                ));
                ui.end_row();
                ui.label("Limites :");
                ui.label(format!(
                    "lecture {}, écriture {}",
                    fmt_limit(app.cfg_read),
                    fmt_limit(app.cfg_write)
                ));
                ui.end_row();
            });
        } else {
            ui.label("Limitation inactive — tous les accès au dossier sont libres.");
        }
    }
}

impl eframe::App for ThrottleApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Monitoring continu : on redessine même sans interaction.
        ctx.request_repaint_after(REFRESH);
        apply_visuals(ctx, self.theme_dark);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Limiteur de débit — dossier");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.checkbox(&mut self.theme_dark, "Fond noir").changed() {
                        apply_visuals(ctx, self.theme_dark);
                    }
                });
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // --- Dossier ---
            ui.label("Dossier à limiter (tous les programmes sont concernés) :");
            ui.add(
                egui::TextEdit::singleline(&mut self.path)
                    .hint_text(r"C:\Chemin\Vers\Le\Dossier")
                    .desired_width(ui.available_width()),
            );
            ui.add_space(8.0);

            // --- Limites ---
            egui::Grid::new("limites")
                .num_columns(3)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Limite lecture :");
                    ui.add(egui::TextEdit::singleline(&mut self.read_val).desired_width(90.0));
                    unit_combo(ui, "unit_lecture", &mut self.read_unit);
                    ui.end_row();
                    ui.label("Limite écriture :");
                    ui.add(egui::TextEdit::singleline(&mut self.write_val).desired_width(90.0));
                    unit_combo(ui, "unit_ecriture", &mut self.write_unit);
                    ui.end_row();
                });
            ui.small("o / Ko / Mo / Go — 0 ou vide = illimité");
            ui.add_space(10.0);

            if self.link.is_none() && !self.active {
                ui.checkbox(&mut self.use_sim, "Mode simulation (sans driver, aucune I/O réelle)");
                ui.add_space(6.0);
            }

            // --- Boutons ---
            ui.horizontal(|ui| {
                let label = if self.active { "Appliquer" } else { "Démarrer" };
                if ui.add(egui::Button::new(label)).clicked() {
                    self.start();
                }
                if ui
                    .add_enabled(self.active, egui::Button::new("Annuler"))
                    .clicked()
                {
                    self.cancel();
                }
            });

            // --- Statut ---
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(2.0);
            Self::status_block(ui, self);
            if let Some(err) = &self.last_error {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::from_rgb(235, 96, 96), err);
            }

            // --- Journal ---
            ui.add_space(8.0);
            ui.separator();
            ui.label("Journal :");
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.log {
                        ui.monospace(line);
                    }
                });
        });

        // --- Monitoring ---
        if self.last_refresh.elapsed() >= REFRESH {
            self.refresh_stats();
        }
    }

    /// À la fermeture : libère les débits côté driver et arrête la simulation.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.active_sim {
            self.stop_sim();
        }
        if let Some(link) = &self.link {
            let msg = ThrottleMessage {
                command: CMD_CLEAR,
                _pad0: 0,
                config: ThrottleConfig::default(),
            };
            let _ = link.send_message(&msg);
        }
    }
}

// ----------------------------------------------------------------------
// Aides
// ----------------------------------------------------------------------

/// Sélecteur d'unité o / Ko / Mo / Go.
fn unit_combo(ui: &mut egui::Ui, id: &str, unit: &mut Unit) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(format!("{}/s", unit.label()))
        .width(76.0)
        .show_ui(ui, |ui| {
            for u in Unit::ALL {
                ui.selectable_value(unit, u, format!("{}/s", u.label()));
            }
        });
}

/// Analyse un nombre saisi à la française (« 10,5 » ou « 10.5 »).
/// Vide = 0.0 (illimité). Renvoie None si non numérique ou négatif.
fn parse_fr(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() {
        return Some(0.0);
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

fn fmt_limit(bps: u64) -> String {
    if bps == 0 {
        "illimitée".to_string()
    } else {
        format!("{}/s", format_bytes(bps as f64))
    }
}

/// Heure locale grossière (UTC) pour horodater le journal.
fn hms_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let day_secs = secs % 86_400;
    format!(
        "[{:02}:{:02}:{:02}]",
        day_secs / 3600,
        (day_secs % 3600) / 60,
        day_secs % 60
    )
}

/// Thème sobre : noir profond ou blanc cassé, bordures fines, aucun accent
/// de couleur superflu. Appliqué sur le thème « Dark » d'egui (unique utilisé).
fn apply_visuals(ctx: &egui::Context, dark: bool) {
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    if dark {
        v.panel_fill = egui::Color32::from_rgb(13, 13, 13);
        v.window_fill = egui::Color32::from_rgb(13, 13, 13);
        v.extreme_bg_color = egui::Color32::from_rgb(22, 22, 22);
        v.faint_bg_color = egui::Color32::from_rgb(19, 19, 19);
        v.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(13, 13, 13);
        v.widgets.noninteractive.fg_stroke.color = egui::Color32::from_rgb(210, 210, 210);
        v.widgets.inactive.bg_fill = egui::Color32::from_rgb(30, 30, 30);
        v.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(225, 225, 225);
        v.widgets.hovered.bg_fill = egui::Color32::from_rgb(42, 42, 42);
        v.widgets.active.bg_fill = egui::Color32::from_rgb(55, 55, 55);
        v.selection.bg_fill = egui::Color32::from_rgb(80, 80, 80);
        v.selection.stroke.color = egui::Color32::from_rgb(200, 200, 200);
    } else {
        v.panel_fill = egui::Color32::from_rgb(250, 250, 250);
        v.window_fill = egui::Color32::from_rgb(250, 250, 250);
        v.extreme_bg_color = egui::Color32::from_rgb(255, 255, 255);
        v.faint_bg_color = egui::Color32::from_rgb(244, 244, 244);
        v.widgets.inactive.bg_fill = egui::Color32::from_rgb(238, 238, 238);
        v.widgets.hovered.bg_fill = egui::Color32::from_rgb(228, 228, 228);
        v.widgets.active.bg_fill = egui::Color32::from_rgb(215, 215, 215);
        v.selection.bg_fill = egui::Color32::from_rgb(200, 200, 200);
    }
    // coins moins arrondis : look utilitaire
    v.window_rounding = egui::Rounding::same(2.0);
    v.widgets.inactive.rounding = egui::Rounding::same(2.0);
    v.widgets.hovered.rounding = egui::Rounding::same(2.0);
    v.widgets.active.rounding = egui::Rounding::same(2.0);
    ctx.style_mut(|style| style.visuals = v);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fr_basique() {
        assert_eq!(parse_fr("10"), Some(10.0));
        assert_eq!(parse_fr("10,5"), Some(10.5));
        assert_eq!(parse_fr("  3.25 "), Some(3.25));
        assert_eq!(parse_fr(""), Some(0.0));
        assert_eq!(parse_fr("   "), Some(0.0));
        assert_eq!(parse_fr("abc"), None);
        assert_eq!(parse_fr("-2"), None);
        assert_eq!(parse_fr("inf"), None);
    }
}
