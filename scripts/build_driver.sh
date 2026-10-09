#!/usr/bin/env bash
# ============================================================================
# build_driver.sh — Recompile le minifilter throttle.sys SOUS LINUX,
# sans WDK ni Visual Studio, pour une cible Windows x64.
#
# Pipeline :
#   1. zig (clang embarqué)  -> compilation C en ABI MSVC, mode noyau
#   2. rust-lld (lld-link)   -> édition de liens /DRIVER /SUBSYSTEM:NATIVE
#   3. osslsigncode          -> signature Authenticode (certificat de test)
#
# Prérequis téléchargés automatiquement (aucun droit root requis) :
#   - zig 0.13.0                        (ziglang.org)
#   - Microsoft.Windows.WDK.x64         (NuGet — headers + libs noyau)
#   - Microsoft.Windows.SDK.cpp         (NuGet — sal.h et includes partagés)
#   - osslsigncode 2.10                 (GitHub — compilé avec gcc)
#
# NB : les headers WDK/SDK sont sensibles à la casse sur Linux alors que les
# #include ne le sont pas sous Windows : scripts/build_case_overlay.py crée
# un arbre de symlinks qui corrige cela.
# ============================================================================
set -euo pipefail

B=/home/z/my-project/.wdkbuild            # répertoire de travail
V=10.0.26100.6584                         # version du WDK NuGet
SV=10.0.26100.9169                        # version du SDK NuGet
SRC=/home/z/my-project/download/throttle-folder/driver-windows/throttle.c
OUT=/home/z/my-project/download/throttle-folder/driver-windows/prebuilt/x64

mkdir -p "$B" && cd "$B"

# --- 1. zig ---------------------------------------------------------------
[ -x zig/zig ] || {
  curl -sL -o zig.tar.xz "https://ziglang.org/download/0.13.0/zig-linux-x86_64-0.13.0.tar.xz"
  tar xf zig.tar.xz && mv zig-linux-x86_64-0.13.0 zig
}

# --- 2. WDK + SDK (NuGet = simples zips de fichiers) ----------------------
[ -d wdk ] || {
  curl -sL -o wdk.nupkg "https://api.nuget.org/v3-flatcontainer/microsoft.windows.wdk.x64/${V}/microsoft.windows.wdk.x64.${V}.nupkg"
  unzip -q wdk.nupkg -d wdk
}
[ -d sdkcpp ] || {
  curl -sL -o sdkcpp.nupkg "https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.cpp/${SV}/microsoft.windows.sdk.cpp.${SV}.nupkg"
  unzip -q sdkcpp.nupkg -d sdkcpp
}

# --- 3. overlay insensible à la casse ------------------------------------
rm -rf overlay
python3 /home/z/my-project/scripts/build_case_overlay.py

# --- 4. compilation -------------------------------------------------------
O="$B/overlay"
./zig/zig cc -target x86_64-windows-msvc -c "$SRC" -o throttle.obj \
  -I"$O/km/crt" -I"$O/km" -I"$O/shared" -I"$O/sdkshared" \
  -D_AMD64_ -DWIN64 -D_WIN64 -DWIN32 -D_WIN32 -D_KERNEL_MODE=1 \
  -DNTDDI_VERSION=0x0A00000B -D_WIN32_WINNT=0x0A00 -DWINVER=0x0A00 \
  -DNDEBUG -fms-compatibility -fms-extensions -fmsc-version=1939 \
  -fno-stack-protector -O2 -w

# --- 5. lien noyau --------------------------------------------------------
# /FILEALIGN:4096 : le noyau refuse de charger un driver dont les données
# brutes sont alignées à 0x200 (0x800701E7 = ERROR_INVALID_ADDRESS, issu de
# STATUS_CONFLICTING_ADDRESSES / STATUS_NOT_MAPPED_DATA) alors qu'un exe
# utilisateur l'accepte. Le WDK aligne les fichiers driver sur 4 Ko.
L="$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
LIBD="$B/wdk/c/Lib/10.0.26100.0/km/x64"
"$L" -flavor link /OUT:throttle.sys /MACHINE:X64 /SUBSYSTEM:NATIVE,10.00 \
  /DRIVER /ENTRY:DriverEntry /NODEFAULTLIB /FILEALIGN:4096 \
  throttle.obj "$LIBD/ntoskrnl.lib" "$LIBD/fltMgr.lib" "$LIBD/hal.lib"

# --- 6. signature (certificat de test auto-signé) ------------------------
[ -x osslsigncode-2.10/osslsigncode ] || {
  curl -sL -o osslsigncode.tar.gz "https://github.com/mtrojnar/osslsigncode/archive/refs/tags/2.10.tar.gz"
  tar xf osslsigncode.tar.gz
  ( cd osslsigncode-2.10 && \
    printf '#define VERSION_MAJOR 2\n#define VERSION_MINOR 10\n#define HAVE_SYS_MMAN_H\n#define HAVE_MMAP\n' > config.h && \
    gcc -O2 -D_GNU_SOURCE -DHAVE_SYS_MMAN_H -DHAVE_MMAP -DHAVE_TERMIOS_H -DHAVE_GETPASS \
        -include sys/mman.h -include unistd.h -o osslsigncode \
        osslsigncode.c helpers.c pe.c msi.c cab.c cat.c appx.c script.c utf.c \
        -I. -lcrypto -lssl -lz )
}
[ -f throttle-test.cer ] || openssl req -x509 -newkey rsa:3072 \
  -keyout throttle-test.key -out throttle-test.cer -days 3650 -nodes \
  -subj "/CN=Throttle Dev Test/O=Local" -addext "extendedKeyUsage=codeSigning"

# osslsigncode refuse d'écraser une sortie existante : re-runs propres.
rm -f throttle-signed.sys
./osslsigncode-2.10/osslsigncode sign -certs throttle-test.cer \
  -key throttle-test.key -h sha256 -n "Throttle Dev Test" -i "https://localhost" \
  -in throttle.sys -out throttle-signed.sys

mkdir -p "$OUT"
cp throttle-signed.sys "$OUT/throttle.sys"
cp throttle-test.cer  "$OUT/throttle-test.cer"

# Le wizard embarque le .sys + le .cer (setup-wizard/embed) : resynchronisation.
EMBED=/home/z/my-project/download/throttle-folder/setup-wizard/embed
mkdir -p "$EMBED"
cp throttle-signed.sys "$EMBED/throttle.sys"
cp throttle-test.cer   "$EMBED/throttle-test.cer"

python3 "$(dirname "$0")/pe_check.py" "$OUT/throttle.sys"
echo "throttle.sys compilé, linké et signé → $OUT"
