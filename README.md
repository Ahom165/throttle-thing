# Limiteur de débit — dossier

Limite la **vitesse de lecture et d'écriture sur un dossier** sous **Windows**,
pour **tous les programmes**, via un **minifilter noyau** piloté par une
interface **Rust + egui** sobre (fond noir ou blanc, champs et boutons).

```
┌────────────────────────────┐        ┌───────────────────────────────┐
│  Application (Rust+egui)   │        │  Minifilter « throttle » (C)  │
│                            │        │                               │
│  Dossier  [C:\...\dossier] │  ───►  │  IRP_MJ_READ  → bucket jetons │
│  Lecture [10] [Mo/s ▾]     │  port  │  IRP_MJ_WRITE → bucket jetons │
│  Écriture [5] [Mo/s ▾]     │ ◄──►   │  → KeDelayExecutionThread     │
│                            │        │    (endort la requête : tout  │
│  [Démarrer]  [Annuler]     │        │     programme est bridé)      │
│  ● Lecture 9,8 Mo/s …      │        └───────────────────────────────┘
└────────────────────────────┘                  │
                                                ▼
                            ┌────────────────────────────────────┐
                            │  NTFS / ReFS — dossier cible       │
                            └────────────────────────────────────┘
```

- **Tous les processus** qui accèdent au dossier sont affectés (Explorateur,
  copies, navigateur, scripts…), sans exception de programme.
- **Annuler** retire la limitation instantanément (aucune I/O conservée).
- Unités **o / Ko / Mo / Go** (base 1024), `0` ou vide = illimité.
- Débits lecture et écriture indépendants.
- Monitoring : débit instantané + totaux, rafraîchis 2×/s.
- Journal d'activité horodaté.

## Contenu du dépôt

```
throttle-folder/
├── throttle-setup.exe       ASSISTANT d'installation (wizard, driver embarqué)
├── throttle-folder.exe     Application Windows x64 précompilée (GUI)
├── setup-wizard/            Sources de l'assistant (Rust + egui, + tests)
├── src/                     Application Rust (interface egui + logique)
│   ├── main.rs              Point d'entrée
│   ├── app.rs               UI sobre (noir/blanc), statut, journal
│   ├── units.rs             Unités o/Ko/Mo/Go + formatage FR (+ tests)
│   ├── token_bucket.rs      Bucket à jetons (miroir Rust du driver) (+ tests)
│   ├── protocol.rs          Structures du protocole app ⇄ driver (+ tests)
│   ├── driver_comm.rs       FilterConnectCommunicationPort / FilterSendMessage
│   └── sim.rs               Mode simulation (démo sans driver)
├── driver-windows/          Minifilter noyau (C)
│   ├── prebuilt/x64/        throttle.sys PRÉCOMPILÉ + signé, install/uninstall .bat
│   ├── throttle.c           Le filtre : pré-ops READ/WRITE, jetons, port
│   ├── throttle.h           Protocole (C_ASSERT = miroir de protocol.rs)
│   ├── throttle.inf         Installation classique (altitude test 399999)
│   ├── install.bat / uninstall.bat
│   ├── BUILD.md             Précompilé (rien à faire), cross-build Linux, WDK
│   └── legacy-build/        Build type EWDK (sources/makefile)
├── scripts/                 Cross-build Linux du driver (clang + lld-link + signature)
└── README.md
```

## 0. Installation guidée — **l'assistant (recommandé)**

Un **assistant d'installation** (`throttle-setup.exe`) est fourni : il embarque
le driver et son certificat de test — **un seul fichier, rien d'autre à
télécharger**. Double-clique dessus :

> Si Windows SmartScreen s'affiche (« Windows a protégé votre PC ») :
> **Informations complémentaires → Exécuter quand même** (normal, l'exe
> n'a pas de réputation de téléchargement).

Ensuite, il n'y a que **deux clics** à faire :

1. Une fenêtre **UAC** s'affiche : accepte (l'assistant s'élève tout seul) ;
2. Clique **Installer**. L'assistant détecte l'état du système puis : installe
   le certificat de test (Racine + Éditeurs approuvés), active le mode
   test-signature si besoin, **désactive l'« Intégrité de la mémoire » (HVCI)
   si elle est active** — Windows 11 bloque les drivers de test tant qu'elle
   est active —, copie le driver dans `C:\ProgramData\ThrottleFolder`, crée le
   service et programme sa **relance automatique après redémarrage** ;
3. Clique **« Redémarrer maintenant »** (ou redémarre plus tard toi-même) ;
4. Après la reprise de session : **l'assistant se relance tout seul**, accepte
   juste l'UAC — il charge le filtre, termine l'installation et propose
   **« Lancer l'application »**. C'est fini.

Le bouton **Désinstaller** décharge le filtre, supprime le service et les
fichiers déployés (le retrait du mode test reste manuel, voir la fin).

## Prérequis (machine Windows)

- Windows 10/11 x64 ;
- **Rien d'autre** : l'application (`throttle-folder.exe`) et le driver
  (`throttle.sys`) sont précompilés ;
- Rust + VS Build Tools seulement si tu veux recompiler l'application,
  WDK seulement si tu veux recompiler le driver (voir `driver-windows/BUILD.md`) ;
- **Droits administrateur** pour le driver et l'application.

## 1. Installer et charger le driver à la main — **rien à compiler**

*(Méthode manuelle — l'assistant du § 0 fait tout cela automatiquement.)*

Un **`throttle.sys` précompilé et signé** est fourni
(`driver-windows/prebuilt/x64/`). Sur ta machine Windows :

```bat
:: une seule fois — machine de test (driver non certifié Microsoft) :
bcdedit /set testsigning on          :: puis REDÉMARRE (filigrane « Test Mode » normal)
```

Puis, en **administrateur**, depuis `driver-windows\prebuilt\x64\` :

```bat
install-noinf.bat                    :: certificat + service + altitude + fltmc load
fltmc filters                        :: « throttle » + altitude 399999
```

Le script installe aussi le certificat de test (`certutil -addstore Root` et
`TrustedPublisher`) — en cas de refus malgré le mode test, vérifie que
`throttle-test.cer` est bien présent dans ces deux boutiques.

> Reconstruire le driver toi-même ? Inutile d'installer le WDK :
> `scripts/build_driver.sh` le recompile **depuis Linux** (clang/lld-link/
> osslsigncode), et `driver-windows/BUILD.md` documente aussi le build
> Visual Studio + WDK classique.

## 2. Compiler et lancer l'application

```bat
cargo build --release                :: cible MSVC standard Windows
target\release\throttle-folder.exe   :: en tant qu'administrateur
```

L'exe attend un **chemin DOS** (`C:\Users\...\dossier`) : l'application le
convertit en forme device (`\Device\HarddiskVolumeN\...`) via `QueryDosDeviceW`
— c'est le préfixe utilisé par le driver pour matcher les noms normalisés,
donc **tout fichier sous ce dossier, ouvert par n'importe quel programme,
est bridé**.

## Utilisation

1. **Dossier à limiter** : chemin du dossier (ex. `C:\Users\moi\Telechargements`).
2. **Limite lecture / écriture** : nombre + unité (`o/s`, `Ko/s`, `Mo/s`,
   `Go/s`) ; `0` ou vide = illimité.
3. **Démarrer** : applique la limitation (le bouton devient « Appliquer » :
   tu peux changer les valeurs à chaud).
4. **Annuler** : libère immédiatement tous les débits.
5. **Fond noir** : bascule thème sombre / clair.
6. **Mode simulation** (visible seulement si le driver est absent) : démontre
   le comportement avec le bucket à jetons Rust, sans aucune I/O réelle —
   utile pour tester l'interface avant d'installer le driver.

## Comment ça bride (détail)

Le driver ne modifie aucune donnée : il **retarde** les opérations.
Pour chaque lecture/écriture sous le dossier cible, l'opération doit acquérir
`taille` octets de jetons dans le bucket de sa direction ; s'il en manque,
`KeDelayExecutionThread` endort la requête le temps de la régénération
(max 1 s par tour, boucle jusqu'à acquit). La rafale autorisée vaut
`max(débit, 64 Kio)` — comme le `TokenBucket` Rust testé unitairement
(`src/token_bucket.rs`), dont l'algorithme est le miroir exact du C.

Exclusions de sécurité (volontaires) :
- requêtes émises **par le noyau** (pagination, cache manager) : les brider
  risque des interblocages système ;
- processus **System** ;
- **l'application elle-même** (PID exclu automatiquement).

Conséquence : les accès « normaux » de tous les programmes sont bien affectés ;
les I/O mémoire-mappées (pagination noyau) ne le sont pas — limite assumée
de la v1.

## Panneaux & dépannage

| Symptôme | Cause probable / remède |
|---|---|
| « Connexion au driver impossible … administrateur » | Lance l'exe **en tant qu'administrateur** |
| « … fltmc load throttle » | Le driver n'est pas chargé → `install.bat` |
| `fltmc load` échoue avec 0xC0000428 | Signature : `bcdedit /set testsigning on` + redémarrage — et sur **Windows 11**, désactive l'« Intégrité de la mémoire » (Sécurité Windows → Isolation du noyau), elle bloque aussi les drivers de test |
| Windows 11 : le mode test est actif mais le driver refuse de se charger | **Intégrité de la mémoire (HVCI) active** → Sécurité Windows → Sécurité de l'appareil → Isolation du noyau → « Intégrité de la mémoire » = **Désactivée** + redémarrage (l'assistant le fait automatiquement) |
| Vitesse non bridée sur une copie | La copie passe par le cache/pagination ou un volume réseau (hors v1) ; essaie avec un gros fichier (> 1 Go) |
| Statut « ⚠ le driver n'est plus actif » | Quelqu'un a fait `fltmc unload throttle` → recharge |

## Limites v1 (assumées)

- Volumes réseau (UNC) non gérés ; volumes locaux uniquement.
- I/O pagination / mmap non bridées (sécurité noyau).
- Altitude de test : à réserver officiellement pour une diffusion.
- Signature de test : mode test-signature requis (voir « Signature du driver »).

## Signature du driver — comment ça marche ?

Depuis Windows 10 1607 (chargement noyau durci), **un driver de filtre doit
être signé par Microsoft** pour se charger « normalement ». Une signature
Authenticode seule — même achetée avec un certificat EV — ne suffit plus
pour un `.sys` noyau. Trois situations :

### 1. Usage perso / test → mode test-signature (ce qui est fourni)

Le `throttle.sys` livré est **déjà signé** (Authenticode SHA-256, certificat
auto-signé `throttle-test.cer`, généré avec `osslsigncode`). Avec ce type de
signature, Windows exige le **mode test** :

```bat
bcdedit /set testsigning on      :: en admin, puis REDÉMARRER
```

Deux précautions rendent le chargement fiable (l'assistant et
`install-noinf.bat` le font pour toi) :
- le **certificat** `throttle-test.cer` doit être installé dans les boutiques
  « Racine » **et** « Éditeurs approuvés » de la machine :
  `certutil -addstore Root throttle-test.cer` puis
  `certutil -addstore -f TrustedPublisher throttle-test.cer` ;
- le filigrane **« Test Mode »** en bas d'écran est normal et sans danger.

Pour ressortir du mode test plus tard :
`bcdedit /set testsigning off` + redémarrage.

### Windows 11 : le vrai obstacle est l'« Intégrité de la mémoire » (HVCI)

Sur Windows 11, le mode test fonctionne **même avec Secure Boot actif**.
En revanche, les installations récentes activent par défaut l'**Intégrité de
la mémoire** (HVCI, dans Sécurité Windows → Isolation du noyau) — et tant
qu'elle est active, **tout driver test-signé est bloqué, même en mode test**.
Deux façons de la couper :

- **Interface** : Sécurité Windows → Sécurité de l'appareil → Isolation du
  noyau → « Intégrité de la mémoire » = Désactivée → redémarrer ;
- **Ligne de commande / assistant** (ce que fait le wizard) :

```bat
reg add "HKLM\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity" /v Enabled /t REG_DWORD /d 0 /f
:: + redémarrage
```

Résumé Windows 11 : `testsigning on` + HVCI **off** + certificat de test dans
Racine/Éditeurs approuvés → le driver se charge. Secure Boot peut rester
activé. (Filigrane « Test Mode » en bas d'écran = normal.)

### 2. Signer avec TON certificat de test

Tu peux remplacer le mien par le tien : crée un certificat de signature de
code, signe le `.sys`, puis distribue ton `.cer` (l'assistant en embarque un
nouveau si tu remplaces `setup-wizard/embed/throttle-test.cer` et recompiles).

**Sous Windows (PowerShell admin) :**

```powershell
# 1. Certificat de test auto-signé (valable 3 ans)
$cer = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=Mon Nom (test)" `
        -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddYears(3)

# 2. Exporter le .cer public et un .pfx privé
Export-Certificate -Cert $cer -FilePath C:\chemin\mon-test.cer
Export-PfxCertificate -Cert $cer -FilePath C:\chemin\mon-test.pfx -Password (ConvertTo-SecureString -String "mdp" -Force -AsPlainText)

# 3. Signer le driver (signtool fait partie du Windows SDK)
signtool sign /fd SHA256 /f C:\chemin\mon-test.pfx /p mdp driver-windows\prebuilt\x64\throttle.sys

# 4. Vérifier
signtool verify /pa /v driver-windows\prebuilt\x64\throttle.sys
```

**Sous Linux** (pipeline déjà en place) : `scripts/build_driver.sh` régénère
un certificat + une signature via `openssl` et `osslsigncode`.

### 3. Distribuer à d'autres machines → signature Microsoft (attestation)

C'est la voie « propre » : Microsoft signe le driver une fois pour toutes,
plus besoin du mode test ni d'installer de certificat chez l'utilisateur.

1. Crée un compte **Microsoft Partner Center** (programme « Matériel
   Windows ») : inscription d'entreprise avec vérification (un **certificat
   EV de signature de code** est requis pour signer le manifeste d'inscription) ;
2. Empaquette le driver soumis : un `.cab` contenant le `.sys` **non signé**
   (+ le `.inf` et les symboles `.pdb`) — via `inf2cat`/`makecab` ou
   directement sur le portail ;
3. Soumets-le en **« attestation signing »** (le portail vérifie l'empreinte
   SHA256 du fichier) ; pas de tests HLK à fournir pour l'attestation ;
4. Quelques heures à jours plus tard, télécharge le `.sys` **signé par
   « Microsoft Windows Hardware Compatibility Publisher »** : il se charge
   partout (Windows 10/11 x64), sans `testsigning`, sans filigrane ;
5. Optionnel : réserve une **altitude officielle** du filtre à Microsoft
   (l'altitude de test `399999` doit être remplacée pour une diffusion).

La voie **WHQL** (avec tests HLK en laboratoire) n'est utile que pour la
certification/logo ; l'attestation suffit pour distribuer.

En résumé :

| Objectif | Solution |
|---|---|
| Utiliser chez toi (Win10/11) | Signature de test fournie + `testsigning on` + HVCI off (l'assistant s'en charge) |
| Signer avec ton cert | `New-SelfSignedCertificate` + `signtool sign /fd SHA256` |
| Distribuer publiquement | Attestation signing via Partner Center |

## Tests

```bat
cargo test --no-default-features    :: 13 tests (unités, bucket, protocole…)
cd setup-wizard && cargo test --no-default-features   :: 8 tests (assistant : état, HVCI, actions)
```

Les garde-fous de layout (`const assert` Rust / `C_ASSERT` C) garantissent que
`ThrottleMessage` fait bien 560 octets des deux côtés : toute divergence
entre `protocol.rs` et `throttle.h` casse une compilation.
