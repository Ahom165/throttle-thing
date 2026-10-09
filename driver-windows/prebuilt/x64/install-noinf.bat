@echo off
REM =====================================================================
REM  install-noinf.bat — Installe et charge le driver PRÉCOMPILÉ
REM  (throttle.sys signé, fourni dans ce dossier), SANS INF ni catalogue.
REM
REM  À exécuter en tant qu'ADMINISTRATEUR, depuis CE dossier.
REM
REM  Prérequis (driver de test, non certifié Microsoft) :
REM     bcdedit /set testsigning on      → puis REDÉMARRER le PC
REM =====================================================================

set SVC=throttle
set SYS=%~dp0throttle.sys
set CER=%~dp0throttle-test.cer

if not exist "%SYS%" (
    echo [ERREUR] throttle.sys introuvable a cote de ce script.
    pause & exit /b 1
)

fltmc unload %SVC% 2>nul

echo Installation du certificat de test (Racine + Editeurs approuves)...
if exist "%CER%" (
    certutil -addstore -f Root "%CER%" >nul 2>&1
    certutil -addstore -f TrustedPublisher "%CER%" >nul 2>&1
    echo Certificat installe.
) else (
    echo [INFO] throttle-test.cer absent : certificat non installe.
)

echo Creation du service noyau...
sc create %SVC% type= filesys start= demand binPath= "%SYS%" DisplayName= "Limiteur de debit dossier (minifilter)" >nul 2>&1
sc config %SVC% start= demand >nul 2>&1
sc description %SVC% "Bridge le debit lecture/ecriture d'un dossier pour tous les processus" >nul 2>&1

echo Enregistrement de l'instance (altitude de test 399999)...
reg add HKLM\SYSTEM\CurrentControlSet\Services\%SVC%\Instances /v DefaultInstance /t REG_SZ /d "throttle - Default Instance" /f >nul
reg add "HKLM\SYSTEM\CurrentControlSet\Services\%SVC%\Instances\throttle - Default Instance" /v Altitude /t REG_SZ /d 399999 /f >nul
reg add "HKLM\SYSTEM\CurrentControlSet\Services\%SVC%\Instances\throttle - Default Instance" /v Flags /t REG_DWORD /d 0 /f >nul

echo Verification de l'Integrite de la memoire (HVCI, Windows 11)...
reg query "HKLM\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity" /v Enabled 2>nul | find "0x1" >nul 2>&1
if not errorlevel 1 (
    echo [INFO] HVCI active : Windows 11 bloque les drivers de test meme en mode test.
    echo        Desactivation de l'Integrite de la memoire...
    reg add "HKLM\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity" /v Enabled /t REG_DWORD /d 0 /f >nul
    echo [OK] HVCI desactive : REDÉMARRE le PC (sinon le chargement echouera).
)

echo Chargement du filtre...
fltmc load %SVC%
if errorlevel 1 (
    echo.
    echo [ERREUR] fltmc load a echoue (code %errorlevel%).
    echo   1. Verifie le mode test-signature : bcdedit /set testsigning on
    echo      (puis REDERMARRE) — obligatoire pour un driver non certifie.
    echo   2. Si besoin, importe le certificat de test :
    echo      certutil -addstore Root "%~dp0throttle-test.cer"
    pause & exit /b 1
)

echo.
echo Termine. Instances attachees :
fltmc instances -f %SVC%
echo.
echo Lance maintenant throttle-folder.exe en tant qu'administrateur.
pause
