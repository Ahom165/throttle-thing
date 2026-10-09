# Compiler le driver minifilter « throttle » (Windows)

Le driver est un **minifilter noyau** écrit en C. **Tu n'as normalement rien
à compiler** : un `throttle.sys` précompilé (x64, signé avec un certificat de
test) est fourni dans `prebuilt/x64/`. Les options ci-dessous ne servent que
si tu veux reconstruire toi-même.

## Option 0 — Driver précompilé (recommandé, zéro installation)

```
driver-windows/prebuilt/x64/
├── throttle.sys            compilé + signé (Authenticode, cert. de test)
├── throttle-test.cer       le certificat (import facultatif)
├── install-noinf.bat       installation sans INF ni catalogue
└── uninstall-noinf.bat
```

1. `bcdedit /set testsigning on` puis **redémarre** (driver de test non
   certifié Microsoft — le filigrane « Test Mode » est normal) ;
2. **install-noinf.bat** en administrateur (crée le service, l'altitude
   399999, puis `fltmc load throttle`) ;
3. si `fltmc load` refuse malgré le mode test : 
   `certutil -addstore Root throttle-test.cer` puis réessaie.

## Option 1 — Compilation croisée depuis Linux (comme le build fourni)

Le `throttle.sys` fourni a été compilé **sous Linux** sans WDK ni Visual
Studio, via `scripts/build_driver.sh` (à la racine du dépôt du projet) :

- **clang** (embarqué dans zig) cible `x86_64-windows-msvc` avec
  `-D_KERNEL_MODE` — compile `throttle.c` contre les headers du WDK ;
- **lld-link** (fourni par Rust) lie en `/DRIVER /SUBSYSTEM:NATIVE`
  contre `ntoskrnl.lib`, `fltMgr.lib`, `hal.lib` du WDK ;
- **osslsigncode** appose la signature Authenticode (certificat auto-signé) ;
- les headers WDK/SDK (NuGet `Microsoft.Windows.WDK.x64`,
  `Microsoft.Windows.SDK.cpp`) sont de simples zips — rien à installer.

Tout est scripté : `bash scripts/build_driver.sh` reconstruit et re-signe
`prebuilt/x64/throttle.sys` à l'identique.

## Option A — Visual Studio 2022 + WDK

1. Installe **Visual Studio 2022** (édition Community suffit) avec la charge
   de travail **« Développement Windows avec C++ »**, puis le
   **Windows Driver Kit (WDK) 10 / 11** correspondant
   (https://learn.microsoft.com/windows-hardware/drivers/download-the-wdk).
2. **Nouveau projet** → recherche « Kernel Mode Driver » →
   **« Kernel Mode Driver, Empty (KMDF) »**. Nom : `throttle`.
3. Supprime les fichiers d'exemple éventuels, puis **Ajouter → Élément
   existant** : `throttle.c` et `throttle.h` (de ce dossier).
4. Dans les propriétés du projet (configuration **Release / x64**) :
   - *Driver Settings → Driver Model* : `WDM` (le modèle KMDF du modèle de
     projet est ignoré : le code est un filtre FltMgr pur) ;
   - *Driver Signing → Test Sign* est le mode par défaut en Debug/Release
     « test » : garde-le pour une installation locale.
5. **Générer → Générer la solution**. Récupère `throttle.sys`
   (dans `x64\Release\`).
6. Copie `throttle.inf` de ce dossier à côté du `.sys`, adapte si besoin le
   nom du `.cat` (VS en génère un ; renomme-le ou ajuste `CatalogFile=`).
7. Installe avec **install.bat** (administrateur).

## Option B — EWDK en ligne de commande

L'« Enterprise WDK » (archive zip sans installation) contient encore
l'ancien système de build. Depuis l'invite fournie par l'EWDK :

```bat
cd driver-windows\legacy-build
copy ..\throttle.c ..\throttle.h .
copy sources makefile .
build -cZg
```

Le `throttle.sys` sort dans `objfre_win10_amd64\amd64\`.

> Les fichiers `sources`/`makefile` sont fournis à titre de commodité dans
> `legacy-build/` ; l'option A reste la voie officielle moderne.

## Signer et charger (machine de test)

Un driver noyau non certifié exige le mode test-signature :

```bat
bcdedit /set testsigning on
:: → redémarre le PC (petit filigrane « Test mode » en bas d'écran)
```

Puis, en **administrateur**, depuis le dossier du driver :

```bat
install.bat       :: installe le service + fltmc load throttle
```

Vérification :

```bat
fltmc filters          :: « throttle » doit apparaître avec son altitude 399999
```

Pour décharger / supprimer :

```bat
uninstall.bat
```

## Altitude

L'INF utilise l'altitude de test **399999**. Pour distribuer publiquement,
demande une altitude officielle à Microsoft et remplace la valeur dans
`throttle.inf` (`Throttle.AddReg`).

## Ce que fait le driver (résumé)

- S'attache à **tous les volumes locaux** ;
- Intercepte **IRP_MJ_READ / IRP_MJ_WRITE** en pré-opération ;
- Si le fichier est **sous le dossier cible** : consomme des jetons
  (bucket à jetons, cf. `throttle.c` — miroir exact de `src/token_bucket.rs`)
  et **endort la requête** via `KeDelayExecutionThread` jusqu'à ce que le
  débit autorisé le permette — le processus appelant est bridé ;
- **Tous les programmes** sont concernés (Explorateur, copies, navigateur...),
  sauf exclusions de sécurité : requêtes émises par le noyau (pagination),
  processus **System** et l'application elle-même ;
- « Annuler » dans l'interface envoie CMD_CLEAR : limitation retirée
  **immédiatement**, aucune I/O en attente conservée.
