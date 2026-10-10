/*
 * throttle.c — Minifilter de limitation de débit lecture/écriture sur un dossier.
 *
 * PRINCIPE
 * --------
 *  - L'application utilisateur (Rust + egui) se connecte au port
 *    de communication "\ThrottlePort" et envoie THROTTLE_MESSAGE :
 *      - CMD_SET_CONFIG : dossier cible (forme device), débits lecture/écriture
 *        en octets/s (0 = illimité), PID de l'application à exclure.
 *      - CMD_CLEAR      : annulation immédiate de toute limitation.
 *      - CMD_GET_STATS  : compteurs cumulés + débits mesurés (réponse synchrone).
 *
 *  - Le filtre s'attache à tous les volumes et intercepte IRP_MJ_READ et
 *    IRP_MJ_WRITE en pré-opération. Si le fichier visé se trouve SOUS le
 *    dossier cible, l'opération doit d'abord consommer des jetons
 *    (bucket à jetons) : tant qu'il n'y en a pas, KeDelayExecutionThread
 *    endort la requête — le processus appelant est donc bridé, quel qu'il
 *    soit (aucune exception de programme, hormis System et l'application).
 *
 *  - La vitesse se limite par ATTENTE : la lecture/écriture "bloque" le temps
 *    nécessaire. Aucune donnée n'est modifiée, aucun fichier n'est intercepté.
 *
 * EXCLUSIONS VOLONTAIRES (sécurité du système, v1)
 * ------------------------------------------------
 *  - Requêtes émises en mode noyau (pagination, cache manager) : NON bridées.
 *    Brider Mm risque des interblocages système. Les accès "normaux" de tous
 *    les programmes (CreateFile/ReadFile/WriteFile,Explorateur, copies...)
 *    passent en mode utilisateur et sont donc bien tous affectés.
 *  - Processus System et processus de l'application elle-même.
 *
 * NOMS DE FICHIERS
 * ----------------
 *  - Le driver compare le préfixe du nom NORMALISÉ du fichier
 *    (\Device\HarddiskVolumeX\dir\...) au dossier cible fourni par
 *    l'application — laquelle convertit « C:\dir » via QueryDosDeviceW.
 *  - En cas d'échec de FltGetFileNameInformation, on ne bride pas (fail-open).
 *
 * DÉBITS
 * ------
 *  - Bucket à jetons par direction (lecture / écriture), indépendants.
 *  - Capacité = max(débit, 64 Kio) : petite rafale tolérée, then bridage.
 *  - Tout est protégé par spinlock ; l'attente se fait HORS spinlock, à
 *    PASSIVE_LEVEL (pré-ops lecture/écriture en mode utilisateur).
 */

#include <fltKernel.h>
#include <dontuse.h>
#include <ntstrsafe.h>
#include "throttle.h"

/* ------------------------------------------------------------------ */
/* État                                                                */
/* ------------------------------------------------------------------ */

typedef struct _THROTTLE_BUCKET {
    KSPIN_LOCK  Lock;
    ULONG64     LastTick;   /* KeQueryInterruptTime, unités de 100 ns */
    ULONG64     Tokens;     /* jetons disponibles (octets)            */
    ULONG64     Capacity;   /* rafale maximale (octets)               */
} THROTTLE_BUCKET, *PTHROTTLE_BUCKET;

typedef struct _THROTTLE_STATE {
    BOOLEAN         Active;
    WCHAR           TargetPath[THROTTLE_PATH_CHARS]; /* UPCASE, '\' final */
    USHORT          TargetPathLen;                   /* en WCHAR          */
    ULONG64         ReadRate;   /* o/s, 0 = illimité */
    ULONG64         WriteRate;
    ULONG64         AppPid;
    THROTTLE_BUCKET ReadBucket;
    THROTTLE_BUCKET WriteBucket;
    /* Compteurs cumulés (interlocked) */
    volatile LONG64 TotalRead;
    volatile LONG64 TotalWrite;
    /* Instantané précédent -> calcul des débits affichés */
    ULONG64         PrevTick;
    LONG64          PrevTotalRead;
    LONG64          PrevTotalWrite;
    ULONG64         ReadBpsShown;
    ULONG64         WriteBpsShown;
} THROTTLE_STATE, *PTHROTTLE_STATE;

static THROTTLE_STATE gState;
static KSPIN_LOCK     gStateLock;      /* protège config + instantanés    */
static PFLT_FILTER    gFilter = NULL;
static PFLT_PORT      gServerPort = NULL;
static PFLT_PORT      gClientPort = NULL;

/* ------------------------------------------------------------------ */
/* Bucket à jetons (miroir C de src/token_bucket.rs)                   */
/* ------------------------------------------------------------------ */

static VOID
ThrottleBucketInit(
    _Out_ PTHROTTLE_BUCKET Bucket
    )
{
    KeInitializeSpinLock(&Bucket->Lock);
    Bucket->LastTick = KeQueryInterruptTime();
    Bucket->Tokens = 0;
    Bucket->Capacity = 0;
}

static VOID
ThrottleBucketSetRate(
    _Inout_ PTHROTTLE_BUCKET Bucket,
    _In_ ULONG64 Rate
    )
{
    Bucket->Capacity = (Rate > (64 * 1024)) ? Rate : (64 * 1024);
    Bucket->Tokens = Bucket->Capacity;  /* rafale initiale complète */
    Bucket->LastTick = KeQueryInterruptTime();
}

/*
 * Attend puis consomme `bytes` jetons. Rate == 0 -> retour immédiat
 * (illimité). Appelable uniquement à PASSIVE_LEVEL (pré-ops user mode).
 */
static VOID
ThrottleAcquire(
    _Inout_ PTHROTTLE_BUCKET Bucket,
    _In_ ULONG64 Rate,
    _In_ ULONG64 Bytes
    )
{
    LARGE_INTEGER delay;
    KIRQL oldIrql;
    ULONG64 now, delta, needed, ms;

    if (Rate == 0 || Bytes == 0) {
        return;
    }

    for (;;) {
        KeAcquireSpinLock(&Bucket->Lock, &oldIrql);

        now = KeQueryInterruptTime();
        delta = now - Bucket->LastTick;
        Bucket->LastTick = now;
        if (delta > 20000000) {         /* clamp à 2 s : borne le calcul     */
            delta = 20000000;
        }
        /* Recharge : delta est en unités de 100 ns (10 000 000 par seconde),
         * Rate en octets/s. On crédite donc (delta * Rate) / 10 000 000.
         * Forme éclatée (partie entière + fractionnaire) : résultat strictement
         * identique à ((delta * Rate) / 10000000ULL) mais sans débordement
         * possible du produit, et précision sub-seconde conservée — les
         * recharges durent typiquement quelques millisecondes, où un simple
         * delta / 10000000 donnerait 0.                                         */
        Bucket->Tokens += (delta / 10000000ULL) * Rate
                        + ((delta % 10000000ULL) * Rate) / 10000000ULL;
        if (Bucket->Tokens > Bucket->Capacity) {
            Bucket->Tokens = Bucket->Capacity;
        }

        if (Bucket->Tokens >= Bytes) {
            Bucket->Tokens -= Bytes;
            KeReleaseSpinLock(&Bucket->Lock, oldIrql);
            return;
        }

        needed = Bytes - Bucket->Tokens;
        KeReleaseSpinLock(&Bucket->Lock, oldIrql);

        /* Attente HORS spinlock, à PASSIVE_LEVEL : c'est elle qui bride.
         * ms = needed * 1000 / rate (avec garde anti-débordement).        */
        if (needed > (1ULL << 54)) {
            ms = 1000;
        } else {
            ms = (needed * 1000) / Rate;
        }
        if (ms < 1) {
            ms = 1;
        }
        if (ms > 1000) {
            ms = 1000;                  /* on reboucle : jamais plus d'1 s   */
        }
        delay.QuadPart = -(LONGLONG)(ms * 10000);   /* relatif, 100 ns    */
        KeDelayExecutionThread(KernelMode, FALSE, &delay);
    }
}

/* ------------------------------------------------------------------ */
/* Aides chemins                                                       */
/* ------------------------------------------------------------------ */

/* Comparaison de préfixe insensible à la casse, 100 % kernel-safe.
 * `target` est déjà upcasé à la réception de la configuration.        */
static BOOLEAN
ThrottlePrefixMatch(
    _In_reads_(NameLen) PCWSTR Name,
    _In_ USHORT NameLen,
    _In_reads_(TargetLen) PCWSTR Target,
    _In_ USHORT TargetLen
    )
{
    USHORT i;

    if (NameLen < TargetLen) {
        return FALSE;
    }
    for (i = 0; i < TargetLen; i++) {
        if (RtlUpcaseUnicodeChar(Name[i]) != Target[i]) {
            return FALSE;
        }
    }
    return TRUE;
}

/*
 * Vérifie que le fichier visé se trouve sous le dossier cible.
 * Fail-open : tout échec de résolution de nom -> pas de bridage.
 */
static BOOLEAN
ThrottlePathMatches(
    _In_ PFLT_CALLBACK_DATA Data
    )
{
    PFLT_FILE_NAME_INFORMATION nameInfo = NULL;
    BOOLEAN match = FALSE;
    NTSTATUS status;
    USHORT targetLen;

    /* Lecture « volatile » : TargetPathLen est publié EN DERNIER par
     * ThrottleMessageNotify (SET_CONFIG) après une barrière mémoire —
     * un lecteur concurrent ne peut donc jamais matcher un buffer
     * à moitié écrit (il voit soit l'ancienne config, soit len == 0). */
    targetLen = *(volatile USHORT *)&gState.TargetPathLen;
    if (targetLen == 0) {
        return FALSE;
    }

    status = FltGetFileNameInformation(
        Data,
        FLT_FILE_NAME_NORMALIZED | FLT_FILE_NAME_QUERY_DEFAULT,
        &nameInfo);
    if (!NT_SUCCESS(status)) {
        return FALSE;
    }

    match = ThrottlePrefixMatch(nameInfo->Name.Buffer,
                                (USHORT)(nameInfo->Name.Length / sizeof(WCHAR)),
                                gState.TargetPath,
                                targetLen);

    FltReleaseFileNameInformation(nameInfo);
    return match;
}

/* Le requête doit-elle être bridée ? (mode user, ni System, ni l'app) */
static BOOLEAN
ThrottleShouldThrottle(
    _In_ PFLT_CALLBACK_DATA Data
    )
{
    if (!gState.Active) {
        return FALSE;
    }
    if (Data->RequestorMode != UserMode) {
        return FALSE;   /* pagination / noyau : hors de portée (sécurité) */
    }
    if (PsGetCurrentProcessId() == (HANDLE)(ULONG_PTR)gState.AppPid) {
        return FALSE;   /* l'application elle-même */
    }
    if (PsGetCurrentProcess() == PsInitialSystemProcess) {
        return FALSE;   /* System */
    }
    return TRUE;
}

/* ------------------------------------------------------------------ */
/* Pré-opérations lecture / écriture                                   */
/* ------------------------------------------------------------------ */

FLT_PREOP_CALLBACK_STATUS
ThrottlePreRead(
    _Inout_ PFLT_CALLBACK_DATA Data,
    _In_ PCFLT_RELATED_OBJECTS FltObjects,
    _Flt_CompletionContext_Out_ PVOID *CompletionContext
    )
{
    ULONG64 length;

    UNREFERENCED_PARAMETER(FltObjects);
    UNREFERENCED_PARAMETER(CompletionContext);

    if (ThrottleShouldThrottle(Data) && gState.ReadRate != 0 &&
        ThrottlePathMatches(Data)) {

        length = Data->Iopb->Parameters.Read.Length;
        InterlockedAdd64(&gState.TotalRead, (LONG64)length);
        ThrottleAcquire(&gState.ReadBucket, gState.ReadRate, length);
    }

    return FLT_PREOP_SUCCESS_NO_CALLBACK;
}

FLT_PREOP_CALLBACK_STATUS
ThrottlePreWrite(
    _Inout_ PFLT_CALLBACK_DATA Data,
    _In_ PCFLT_RELATED_OBJECTS FltObjects,
    _Flt_CompletionContext_Out_ PVOID *CompletionContext
    )
{
    ULONG64 length;

    UNREFERENCED_PARAMETER(FltObjects);
    UNREFERENCED_PARAMETER(CompletionContext);

    if (ThrottleShouldThrottle(Data) && gState.WriteRate != 0 &&
        ThrottlePathMatches(Data)) {

        length = Data->Iopb->Parameters.Write.Length;
        InterlockedAdd64(&gState.TotalWrite, (LONG64)length);
        ThrottleAcquire(&gState.WriteBucket, gState.WriteRate, length);
    }

    return FLT_PREOP_SUCCESS_NO_CALLBACK;
}

/* ------------------------------------------------------------------ */
/* Communication utilisateur                                           */
/* ------------------------------------------------------------------ */

static NTSTATUS
ThrottleMessageNotify(
    _In_opt_ PVOID PortCookie,
    _In_reads_bytes_opt_(InputBufferLength) PVOID InputBuffer,
    _In_ ULONG InputBufferLength,
    _Out_writes_bytes_to_opt_(OutputBufferLength, *ReturnOutputLength) PVOID OutputBuffer,
    _In_ ULONG OutputBufferLength,
    _Out_ PULONG ReturnOutputLength
    )
{
    PTHROTTLE_MESSAGE msg = (PTHROTTLE_MESSAGE)InputBuffer;
    KIRQL oldIrql;
    ULONG n;

    UNREFERENCED_PARAMETER(PortCookie);
    UNREFERENCED_PARAMETER(OutputBufferLength);

    if (InputBuffer == NULL || InputBufferLength < sizeof(ULONG)) {
        return STATUS_INVALID_PARAMETER;
    }

    switch (msg->Command) {

    case THROTTLE_CMD_SET_CONFIG:
        if (InputBufferLength < sizeof(THROTTLE_MESSAGE)) {
            return STATUS_INVALID_PARAMETER;
        }
        /* longueur du chemin fourni, en WCHAR, sans le nul */
        for (n = 0; n < THROTTLE_PATH_CHARS; n++) {
            if (msg->Config.TargetPath[n] == L'\0') {
                break;
            }
        }
        if (n == 0 || n >= THROTTLE_PATH_CHARS - 2) {
            return STATUS_INVALID_PARAMETER;
        }

        KeAcquireSpinLock(&gStateLock, &oldIrql);

        /* Publication en deux temps (les pré-ops lisent la config SANS le
         * verrou) : longueur à 0 d'abord (plus de matching pendant la copie),
         * écriture du buffer, puis longueur EN DERNIER après barrière.
         * Un lecteur voit soit l'ancienne config, soit la nouvelle complète
         * — jamais un état intermédiaire à moitié écrit. */
        *(volatile USHORT *)&gState.TargetPathLen = 0;
        KeMemoryBarrier();

        RtlZeroMemory(gState.TargetPath, sizeof(gState.TargetPath));
        RtlCopyMemory(gState.TargetPath,
                      msg->Config.TargetPath,
                      n * sizeof(WCHAR));
        if (gState.TargetPath[n - 1] != L'\\') {
            gState.TargetPath[n++] = L'\\';     /* '\' final obligatoire   */
        }
        gState.TargetPath[n] = L'\0';
        {
            ULONG i;
            for (i = 0; i < n; i++) {
                gState.TargetPath[i] = RtlUpcaseUnicodeChar(gState.TargetPath[i]);
            }
        }

        gState.ReadRate = msg->Config.ReadBytesPerSec;
        gState.WriteRate = msg->Config.WriteBytesPerSec;
        gState.AppPid = msg->Config.AppPid;
        gState.Active = (msg->Config.Enabled != 0);

        ThrottleBucketSetRate(&gState.ReadBucket, gState.ReadRate);
        ThrottleBucketSetRate(&gState.WriteBucket, gState.WriteRate);

        gState.PrevTick = KeQueryInterruptTime();
        gState.PrevTotalRead = gState.TotalRead;
        gState.PrevTotalWrite = gState.TotalWrite;
        gState.ReadBpsShown = 0;
        gState.WriteBpsShown = 0;

        /* Dernier : la nouvelle config devient visible pour les lecteurs
         * qui ne prennent pas le verrou (ThrottlePathMatches). */
        KeMemoryBarrier();
        *(volatile USHORT *)&gState.TargetPathLen = (USHORT)n;

        KeReleaseSpinLock(&gStateLock, oldIrql);

        DbgPrintEx(DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL,
                   "throttle: config — %ls | lecture %llu o/s, ecriture %llu o/s, active=%u\n",
                   gState.TargetPath,
                   gState.ReadRate,
                   gState.WriteRate,
                   gState.Active ? 1u : 0u);
        break;

    case THROTTLE_CMD_CLEAR:
        KeAcquireSpinLock(&gStateLock, &oldIrql);
        gState.Active = FALSE;
        /* Longueur à 0 d'abord : les lecteurs sans verrou cessent de
         * matcher immédiatement, même avant la fin de la remise à zéro. */
        *(volatile USHORT *)&gState.TargetPathLen = 0;
        KeMemoryBarrier();
        gState.TargetPath[0] = L'\0';
        gState.ReadRate = 0;
        gState.WriteRate = 0;
        gState.ReadBpsShown = 0;
        gState.WriteBpsShown = 0;
        KeReleaseSpinLock(&gStateLock, oldIrql);
        DbgPrintEx(DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL,
                   "throttle: limitation annulee\n");
        break;

    case THROTTLE_CMD_GET_STATS:
        if (OutputBuffer == NULL ||
            OutputBufferLength < sizeof(THROTTLE_STATS) ||
            ReturnOutputLength == NULL) {
            return STATUS_INVALID_PARAMETER;
        }

        KeAcquireSpinLock(&gStateLock, &oldIrql);
        {
            ULONG64 now = KeQueryInterruptTime();
            ULONG64 delta = now - gState.PrevTick;
            if (delta == 0) {
                delta = 1;
            }
            gState.ReadBpsShown =
                (ULONG64)(gState.TotalRead - gState.PrevTotalRead) * 10000000 / delta;
            gState.WriteBpsShown =
                (ULONG64)(gState.TotalWrite - gState.PrevTotalWrite) * 10000000 / delta;
            gState.PrevTick = now;
            gState.PrevTotalRead = gState.TotalRead;
            gState.PrevTotalWrite = gState.TotalWrite;
        }

        {
            THROTTLE_STATS out;
            out.TotalBytesRead = (ULONG64)gState.TotalRead;
            out.TotalBytesWritten = (ULONG64)gState.TotalWrite;
            out.ReadBytesPerSec = gState.ReadBpsShown;
            out.WriteBytesPerSec = gState.WriteBpsShown;
            out.Active = gState.Active ? 1 : 0;
            out.Reserved0 = 0;
            KeReleaseSpinLock(&gStateLock, oldIrql);
            RtlCopyMemory(OutputBuffer, &out, sizeof(out));
            *ReturnOutputLength = sizeof(out);
        }
        break;

    default:
        return STATUS_INVALID_PARAMETER;
    }

    return STATUS_SUCCESS;
}

static NTSTATUS
ThrottleConnectNotify(
    _In_ PFLT_PORT ClientPort,
    _In_opt_ PVOID ServerPortCookie,
    _In_reads_bytes_opt_(SizeOfContext) PVOID ConnectionContext,
    _In_ ULONG SizeOfContext,
    _Outptr_result_maybenull_ PVOID *ConnectionCookie
    )
{
    UNREFERENCED_PARAMETER(ServerPortCookie);
    UNREFERENCED_PARAMETER(ConnectionContext);
    UNREFERENCED_PARAMETER(SizeOfContext);
    UNREFERENCED_PARAMETER(ConnectionCookie);

    gClientPort = ClientPort;
    DbgPrintEx(DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL,
               "throttle: client connecte\n");
    return STATUS_SUCCESS;
}

/* Déconnexion du client (application fermée proprement, plantée ou tuée) :
 * la limitation est levée IMMÉDIATEMENT. Sans ça, une application qui
 * disparaît sans envoyer CMD_CLEAR laisserait le dossier bridé pour TOUS
 * les processus jusqu'au redémarrage — plus aucun programme ne pourrait
 * remettre l'état à zéro. */
static VOID
ThrottleDisconnectNotify(
    _In_opt_ PVOID ConnectionCookie
    )
{
    KIRQL oldIrql;

    UNREFERENCED_PARAMETER(ConnectionCookie);

    KeAcquireSpinLock(&gStateLock, &oldIrql);
    gState.Active = FALSE;
    *(volatile USHORT *)&gState.TargetPathLen = 0;
    KeMemoryBarrier();
    gState.TargetPath[0] = L'\0';
    gState.ReadRate = 0;
    gState.WriteRate = 0;
    gState.ReadBpsShown = 0;
    gState.WriteBpsShown = 0;
    KeReleaseSpinLock(&gStateLock, oldIrql);

    gClientPort = NULL;
    DbgPrintEx(DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL,
               "throttle: client deconnecte — limitation levee\n");
}

/* ------------------------------------------------------------------ */
/* Cycle de vie du filtre                                              */
/* ------------------------------------------------------------------ */

NTSTATUS
ThrottleInstanceSetup(
    _In_ PCFLT_RELATED_OBJECTS FltObjects,
    _In_ FLT_INSTANCE_SETUP_FLAGS Flags,
    _In_ DEVICE_TYPE VolumeDeviceType,
    _In_ FLT_FILESYSTEM_TYPE VolumeFilesystemType
    )
{
    UNREFERENCED_PARAMETER(FltObjects);
    UNREFERENCED_PARAMETER(VolumeFilesystemType);

    /* Attachement automatique à tous les volumes locaux. */
    if (Flags & FLTFL_INSTANCE_SETUP_MANUAL_ATTACHMENT) {
        return STATUS_FLT_DO_NOT_ATTACH;
    }
    if (VolumeDeviceType == FILE_DEVICE_NETWORK_FILE_SYSTEM) {
        return STATUS_FLT_DO_NOT_ATTACH;    /* volumes réseau : hors v1 */
    }
    return STATUS_SUCCESS;
}

NTSTATUS
ThrottleUnload(
    _In_ FLT_FILTER_UNLOAD_FLAGS Flags
    )
{
    UNREFERENCED_PARAMETER(Flags);

    if (gClientPort != NULL) {
        FltCloseClientPort(gFilter, &gClientPort);
        gClientPort = NULL;
    }
    if (gServerPort != NULL) {
        FltCloseCommunicationPort(gServerPort);
        gServerPort = NULL;
    }
    if (gFilter != NULL) {
        FltUnregisterFilter(gFilter);
        gFilter = NULL;
    }
    return STATUS_SUCCESS;
}

static const FLT_OPERATION_REGISTRATION ThrottleCallbacks[] = {
    { IRP_MJ_READ,  0, ThrottlePreRead,  NULL },
    { IRP_MJ_WRITE, 0, ThrottlePreWrite, NULL },
    { IRP_MJ_OPERATION_END }
};

static const FLT_CONTEXT_REGISTRATION ThrottleContextRegistration[] = {
    { FLT_CONTEXT_END }
};

static const FLT_REGISTRATION ThrottleFilterRegistration = {
    sizeof(FLT_REGISTRATION),       /* Size                        */
    FLT_REGISTRATION_VERSION,       /* Version                     */
    0,                              /* Flags                       */
    ThrottleContextRegistration,    /* Context                     */
    ThrottleCallbacks,              /* Operation                   */
    ThrottleUnload,                 /* FilterUnloadCallback        */
    ThrottleInstanceSetup,          /* InstanceSetupCallback       */
    NULL,                           /* InstanceQueryTeardownCallback */
    NULL,                           /* InstanceTeardownStartCallback */
    NULL,                           /* InstanceTeardownCompleteCallback */
    NULL,                           /* GenerateFileNameCallback    */
    NULL,                           /* NormalizeNameComponentCallback */
    NULL,                           /* NormalizeNameComponentCleanupCallback */
    NULL,                           /* TransactionNotificationCallback */
    NULL,                           /* OperationStatusCallback     */
    NULL                            /* SectionNotificationCallback */
};

NTSTATUS
DriverEntry(
    _In_ PDRIVER_OBJECT DriverObject,
    _In_ PUNICODE_STRING RegistryPath
    )
{
    NTSTATUS status;
    OBJECT_ATTRIBUTES oa;
    UNICODE_STRING portName;
    PSECURITY_DESCRIPTOR sd = NULL;

    UNREFERENCED_PARAMETER(RegistryPath);

    RtlZeroMemory(&gState, sizeof(gState));
    KeInitializeSpinLock(&gStateLock);
    ThrottleBucketInit(&gState.ReadBucket);
    ThrottleBucketInit(&gState.WriteBucket);

    status = FltRegisterFilter(DriverObject, &ThrottleFilterRegistration, &gFilter);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    /*
     * Port de communication. Le descripteur par défaut n'autorise que
     * administrateurs + système : l'application doit être lancée en admin.
     */
    status = FltBuildDefaultSecurityDescriptor(&sd, FLT_PORT_ALL_ACCESS);
    if (!NT_SUCCESS(status)) {
        FltUnregisterFilter(gFilter);
        gFilter = NULL;
        return status;
    }

    RtlInitUnicodeString(&portName, THROTTLE_PORT_NAME);
    InitializeObjectAttributes(&oa,
                               &portName,
                               OBJ_KERNEL_HANDLE | OBJ_CASE_INSENSITIVE,
                               NULL,
                               sd);

    status = FltCreateCommunicationPort(gFilter,
                                        &gServerPort,
                                        &oa,
                                        NULL,
                                        ThrottleConnectNotify,
                                        ThrottleDisconnectNotify,
                                        ThrottleMessageNotify,
                                        1);
    FltFreeSecurityDescriptor(sd);

    if (!NT_SUCCESS(status)) {
        FltUnregisterFilter(gFilter);
        gFilter = NULL;
        return status;
    }

    status = FltStartFiltering(gFilter);
    if (!NT_SUCCESS(status)) {
        ThrottleUnload(0);
        return status;
    }

    DbgPrintEx(DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL,
               "throttle: filtre charge — port %ls pret\n", THROTTLE_PORT_NAME);
    return STATUS_SUCCESS;
}
