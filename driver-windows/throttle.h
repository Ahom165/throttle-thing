/*
 * throttle.h — Protocole et déclarations du minifilter « throttle ».
 *
 * Mise en correspondance EXACTE avec src/protocol.rs (application Rust).
 * Layouts vérifiés par C_ASSERT côté C et const assert côté Rust :
 * toute divergence fait échouer une des deux compilations.
 */

#pragma once

#include <ntddk.h>

/* --- Commandes utilisateur -> driver (identiques à protocol.rs) --- */
#define THROTTLE_CMD_SET_CONFIG     1
#define THROTTLE_CMD_CLEAR          2
#define THROTTLE_CMD_GET_STATS      3

/* --- Tailles partagées --- */
#define THROTTLE_PATH_CHARS         260     /* MAX_PATH */

/* Débit maximal accepté (1 To/s) : borne le calcul de recharge du bucket
 * (delta * Rate doit rester sans débordement, même si l'application envoie
 * une valeur aberrante — l'utilisateur peut saisir n'importe quoi). */
#define THROTTLE_MAX_RATE           0x0000E8D4A51000ULL   /* 10^12 o/s */

/* Annotation SAL du prototype des pré-opérations (absente de certaines
 * versions du WDK) : repli sûr sur _Out_. */
#ifndef _Flt_CompletionContext_Out_
#define _Flt_CompletionContext_Out_ _Out_
#endif

/* --- Structures partagées (packing natif x64, pas de #pragma pack) --- */

typedef struct _THROTTLE_CONFIG {
    ULONG   Enabled;            /* 1 = limitation active, 0 = inactive          */
    ULONG   Reserved0;
    ULONG64 ReadBytesPerSec;    /* octets/s autorisés, 0 = illimité             */
    ULONG64 WriteBytesPerSec;   /* octets/s autorisés, 0 = illimité             */
    ULONG64 AppPid;             /* PID exclu (l'application elle-même)          */
    WCHAR   TargetPath[THROTTLE_PATH_CHARS]; /* \Device\HarddiskVolumeX\dir\ */
} THROTTLE_CONFIG, *PTHROTTLE_CONFIG;

typedef struct _THROTTLE_MESSAGE {
    ULONG   Command;
    ULONG   Reserved0;
    THROTTLE_CONFIG Config;
} THROTTLE_MESSAGE, *PTHROTTLE_MESSAGE;

typedef struct _THROTTLE_STATS {
    ULONG64 TotalBytesRead;
    ULONG64 TotalBytesWritten;
    ULONG64 ReadBytesPerSec;    /* mesuré depuis le dernier GET_STATS           */
    ULONG64 WriteBytesPerSec;
    ULONG   Active;
    ULONG   Reserved0;
} THROTTLE_STATS, *PTHROTTLE_STATS;

/* Garde-fous — doivent refléter les const assert de protocol.rs. */
C_ASSERT(sizeof(THROTTLE_CONFIG) == 552);
C_ASSERT(sizeof(THROTTLE_MESSAGE) == 560);
C_ASSERT(sizeof(THROTTLE_STATS) == 40);

/* --- Nom du port de communication (identique à driver_comm.rs) --- */
#define THROTTLE_PORT_NAME          L"\\ThrottlePort"

/* --- Déclarations du filtre --- */

DRIVER_INITIALIZE DriverEntry;

FLT_PREOP_CALLBACK_STATUS
ThrottlePreRead(
    _Inout_ PFLT_CALLBACK_DATA Data,
    _In_ PCFLT_RELATED_OBJECTS FltObjects,
    _Flt_CompletionContext_Out_ PVOID *CompletionContext
    );

FLT_PREOP_CALLBACK_STATUS
ThrottlePreWrite(
    _Inout_ PFLT_CALLBACK_DATA Data,
    _In_ PCFLT_RELATED_OBJECTS FltObjects,
    _Flt_CompletionContext_Out_ PVOID *CompletionContext
    );

NTSTATUS
ThrottleInstanceSetup(
    _In_ PCFLT_RELATED_OBJECTS FltObjects,
    _In_ FLT_INSTANCE_SETUP_FLAGS Flags,
    _In_ DEVICE_TYPE VolumeDeviceType,
    _In_ FLT_FILESYSTEM_TYPE VolumeFilesystemType
    );

NTSTATUS
ThrottleUnload(
    _In_ FLT_FILTER_UNLOAD_FLAGS Flags
    );
