#!/usr/bin/env python3
"""Audit PE strict pour un driver noyau Windows x64.

Vérifie tout ce que le chargeur noyau exige et qui a déjà coûté des heures
de débogage (erreur fltmc 0x800701E7 = STATUS_CONFLICTING_ADDRESSES /
STATUS_NOT_MAPPED_DATA) :

  - PE32+, SUBSYSTEM NATIVE, entry point présent
  - IMAGE_FILE_DLL présent : un .sys est une image « DLL » NATIVE ; sans
    ce bit le noyau traite l'image comme un exe à adresse fixe et la
    recharge avec STATUS_CONFLICTING_ADDRESSES (fltmc : 0x800701E7)
  - SectionAlignment == FileAlignment == 0x1000 (le noyau mappe l'image
    page par page : un fichier driver aligné à 0x200 est refusé au
    chargement, contrairement à un exe utilisateur)
  - SizeOfHeaders cohérent, SizeOfImage cohérent
  - BASERELOC présent (le noyau exige un driver relogeable, KASLR)
  - relocalisations DIR64 valides (offsets dans l'image)
  - checksum PE valide (le noyau le vérifie pour les images noyau)
  - table de certificats (Authenticode) présente
  - imports : DLL cibles plausible noyau (ntoskrnl.exe, FLTMGR.SYS, ...)
"""
import struct
import sys

SUBSYS = {1: "NATIVE (kernel)", 2: "WINDOWS_GUI", 3: "WINDOWS_CUI"}
MACH = {0x8664: "x64", 0x14C: "x86", 0xAA64: "ARM64"}

DDIR_NAMES = [
    "Export", "Import", "Resource", "Exception", "Security", "BaseReloc",
    "Debug", "Arch", "GlobalPtr", "TLS", "LoadConfig", "BoundImport",
    "IAT", "DelayImport", "COM", "Reserved",
]

errors: list[str] = []


def fail(msg: str) -> None:
    errors.append(msg)
    print(f"  [ERREUR] {msg}")


def ok(msg: str) -> None:
    print(f"  [ok] {msg}")


def pe_checksum(data: bytearray, cksum_off: int) -> int:
    work = bytearray(data)
    work[cksum_off:cksum_off + 4] = b"\0\0\0\0"
    s = 0
    for i in range(0, len(work) - 1, 2):
        s += struct.unpack_from("<H", work, i)[0]
        s = (s & 0xFFFF) + (s >> 16)
    s = (s & 0xFFFF) + (s >> 16)
    return (s + len(work)) & 0xFFFFFFFF


def main(path: str) -> None:
    data = bytearray(open(path, "rb").read())
    pe_off = struct.unpack_from("<I", data, 0x3C)[0]
    assert data[pe_off:pe_off + 4] == b"PE\0\0", "signature PE absente"
    coff = pe_off + 4
    machine, nsec, _ts, _sym, _nsym, opt_size, coff_chars = struct.unpack_from(
        "<HHIIIHH", data, coff)
    opt = coff + 20
    magic = struct.unpack_from("<H", data, opt)[0]
    assert magic == 0x20B, "pas un PE32+"

    entry_rva = struct.unpack_from("<I", data, opt + 16)[0]
    image_base = struct.unpack_from("<Q", data, opt + 24)[0]
    sec_align, file_align = struct.unpack_from("<II", data, opt + 32)
    # Layout PE32+ réel : OSVer@40, ImageVer@44, SubsysVer@48, Win32Ver@52,
    # SizeOfImage@56, SizeOfHeaders@60, CheckSum@64, Subsystem@68, DllChar@70.
    os_ver = struct.unpack_from("<HH", data, opt + 40)
    img_ver = struct.unpack_from("<HH", data, opt + 44)
    sub_ver = struct.unpack_from("<HH", data, opt + 48)
    sizeof_image = struct.unpack_from("<I", data, opt + 56)[0]
    sizeof_headers = struct.unpack_from("<I", data, opt + 60)[0]
    cksum = struct.unpack_from("<I", data, opt + 64)[0]
    subsystem = struct.unpack_from("<H", data, opt + 68)[0]
    dllchar = struct.unpack_from("<H", data, opt + 70)[0]
    ndd = struct.unpack_from("<I", data, opt + 108)[0]
    ddir = opt + 112

    print(f"Machine     : 0x{machine:04X} ({MACH.get(machine, '?')})")
    print(f"Subsystem   : {subsystem} ({SUBSYS.get(subsystem, '?')})")
    print(f"Entry RVA   : 0x{entry_rva:X}  |  Image base 0x{image_base:X}")
    print(f"Versions    : OS {os_ver[0]}.{os_ver[1]}, image {img_ver[0]}.{img_ver[1]}, subsystem {sub_ver[0]}.{sub_ver[1]}")
    print(f"Alignements : section 0x{sec_align:X}, fichier 0x{file_align:X}")
    print(f"Tailles     : image 0x{sizeof_image:X}, headers 0x{sizeof_headers:X}")
    print(f"DLLCHAR     : 0x{dllchar:04X}")
    print(f"COFF chars  : 0x{coff_chars:04X}")
    lmajor, lminor = data[opt + 2], data[opt + 3]
    print(f"Linker      : {lmajor}.{lminor}")

    # --- exigences noyau ---------------------------------------------------
    if machine != 0x8664:
        fail(f"machine {machine:#x} != x64")
    if subsystem != 1:
        fail("SUBSYSTEM doit être NATIVE (1) pour un driver")
    if entry_rva == 0:
        fail("pas d'entry point")
    if sec_align != 0x1000 or file_align != 0x1000:
        fail("SectionAlignment et FileAlignment doivent valoir 0x1000 "
             "(le noyau refuse un driver dont le fichier est aligné à 0x200)")
    if not (coff_chars & 0x0002):
        fail("COFF: IMAGE_FILE_EXECUTABLE_IMAGE absent")
    if not (coff_chars & 0x2000):
        fail("COFF: IMAGE_FILE_DLL absent : un .sys doit être une image DLL "
             "(0x2022+, lier avec /DLL) sinon le noyau renvoie "
             "STATUS_CONFLICTING_ADDRESSES (0x800701E7)")
    if ndd != 16:
        fail(f"NumberOfRvaAndSizes = {ndd} (16 attendu)")

    # sections
    sec_off = opt + opt_size
    secs = []
    print("Sections    :")
    for i in range(nsec):
        name, vsize, vaddr, rsize, roff = struct.unpack_from(
            "<8sIIII", data, sec_off + 40 * i)
        name = name.rstrip(b"\0").decode()
        chars = struct.unpack_from("<I", data, sec_off + 40 * i + 36)[0]
        secs.append((name, vaddr, vsize, rsize, roff))
        print(f"  {name:8s} VA=0x{vaddr:05X} VSize=0x{vsize:05X} "
              f"Raw=0x{roff:05X} RawSize=0x{rsize:05X} flags=0x{chars:08X}")
        if vaddr % sec_align:
            fail(f"section {name}: VA non alignée")
        if rsize and roff % file_align:
            fail(f"section {name}: PointerToRawData non aligné")
        if rsize and roff + rsize > len(data):
            fail(f"section {name}: données brutes hors fichier")
        if vaddr + vsize > sizeof_image:
            fail(f"section {name}: dépasse SizeOfImage")

    def rva2off(rva: int):
        for _n, vaddr, vsize, rsize, roff in secs:
            if vaddr <= rva < vaddr + max(vsize, rsize):
                return roff + (rva - vaddr)
        return None

    # répertoires de données
    dirs = []
    for i in range(ndd):
        rva, size = struct.unpack_from("<II", data, ddir + 8 * i)
        dirs.append((rva, size))
        if rva or size:
            print(f"DDIR [{i:2d}] {DDIR_NAMES[i]:12s} RVA=0x{rva:05X} "
                  f"size=0x{size:X}")

    if dirs[5][0] == 0 or dirs[5][1] == 0:
        fail("BASERELOC absent : le noyau exige un driver relogeable")
    else:
        # relocalisations DIR64 dans les bornes de l'image ?
        off = rva2off(dirs[5][0])
        end = off + dirs[5][1]
        n_entries = 0
        while off < end:
            page, blocksz = struct.unpack_from("<II", data, off)
            if blocksz == 0:
                break
            for i in range((blocksz - 8) // 2):
                e = struct.unpack_from("<H", data, off + 8 + 2 * i)[0]
                typ, delta = e >> 12, e & 0xFFF
                if typ == 0:
                    continue
                n_entries += 1
                if typ != 10:
                    fail(f"reloc type {typ} != DIR64")
                if page + delta + 8 > sizeof_image:
                    fail(f"reloc hors image: 0x{page + delta:X}")
            off += blocksz
        if n_entries == 0:
            fail("BASERELOC vide")
        else:
            ok(f"relocalisations : {n_entries} entrées DIR64 valides")

    if dirs[4][0] == 0 or dirs[4][1] == 0:
        fail("table de certificats (Authenticode) absente : driver non signé")
    else:
        cert_off, cert_size = dirs[4]
        if cert_off + cert_size > len(data):
            fail("table de certificats hors fichier")
        else:
            ok(f"signature Authenticode présente ({cert_size} octets)")

    # checksum (le noyau le vérifie pour les images kernel)
    computed = pe_checksum(data, opt + 64)
    if computed != cksum:
        fail(f"checksum PE invalide (en-tête 0x{cksum:X}, calculé 0x{computed:X})")
    else:
        ok(f"checksum PE valide (0x{cksum:X})")

    # imports : DLL noyau uniquement
    imp_rva, _imp_size = dirs[1]
    print("Imports     :")
    if imp_rva:
        off = rva2off(imp_rva)
        while off:
            ilt, _ts, _fc, name_rva, _ft = struct.unpack_from("<IIIII", data, off)
            if name_rva == 0:
                break
            noff = rva2off(name_rva)
            dll = data[noff:data.index(b"\0", noff)].decode()
            print(f"  - {dll}")
            if dll.lower() not in ("ntoskrnl.exe", "fltmgr.sys", "hal.dll",
                                   "ndis.sys", "ksecdd.sys", "ntoskrnl.lib"):
                fail(f"import inattendu pour un driver: {dll}")
            off += 20

    print()
    if errors:
        print(f"=== ÉCHEC : {len(errors)} problème(s) ===")
        sys.exit(1)
    print("=== AUDIT PE OK ===")


if __name__ == "__main__":
    main(sys.argv[1])
