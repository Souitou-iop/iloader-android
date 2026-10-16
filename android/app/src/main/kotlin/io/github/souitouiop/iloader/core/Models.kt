package io.github.souitouiop.iloader.core

/** Mirrors the serde shape of the Rust types (camelCase JSON on desktop). */
data class DeviceInfo(
    val name: String,
    val id: Int,
    val udid: String,
    val connectionType: String,
    val version: String,
)

data class CertificateInfo(
    val name: String?,
    val certificateId: String?,
    val serialNumber: String?,
    val machineName: String?,
    val machineId: String?,
)

data class AppIdInfo(
    val appIdId: String,
    val identifier: String,
    val name: String,
    /** ISO-8601 date time, null when the App ID never expires. */
    val expirationDate: String?,
)

data class ListAppIdsResponse(
    val appIds: List<AppIdInfo>,
    val maxQuantity: Int,
    val availableQuantity: Int,
)

data class PairingAppInfo(
    val name: String,
    val bundleId: String,
    val path: String,
)

/** The pairing file bytes and the name the UI should suggest when saving. */
class PairingExport(val bytes: ByteArray, val fileName: String)

/** Result of one entry of `listDevices`: `Ok(DeviceInfo)` or `Err(AppError)`. */
sealed interface DeviceResult {
    data class Ok(val device: DeviceInfo) : DeviceResult
    data class Err(val error: IloaderException) : DeviceResult
}

/** `AppError` as it crosses the bridge: `{ "type": "<snake_case variant>", "message": "..." }`. */
class IloaderException(val type: String, override val message: String) : Exception(message)

/** Parameters of the 2FA prompt. Empty for now, the Rust side sends no data the UI uses. */
class TwoFactorParams

data class LogRecord(
    /** 1 trace, 2 debug, 3 info, 4 warn, 5 error. */
    val level: Int,
    val message: String,
    val target: String?,
    val timestamp: String,
) {
    companion object {
        const val TRACE = 1
        const val DEBUG = 2
        const val INFO = 3
        const val WARN = 4
        const val ERROR = 5
    }
}

enum class UpdateType { Started, Finished, Failed }

data class OperationUpdate(
    val updateType: UpdateType,
    val stepId: String,
    val extraDetails: IloaderException? = null,
)

/** State of the iPhone pairing (trust prompt) as reported by the USB host. */
sealed interface PairState {
    data object Pairing : PairState
    data object WaitingForTrust : PairState
    data object Paired : PairState
    data object Denied : PairState
    data class Failed(val message: String) : PairState
}

data class PairEvent(val udid: String, val state: PairState)

/** Events pushed by the core, see `CoreEvent` in the contract. */
sealed interface CoreEvent {
    data class TwoFactorRequired(val params: TwoFactorParams) : CoreEvent
    data class MaxCertsReached(val certificates: List<CertificateInfo>) : CoreEvent
    data class Operation(val operationId: String, val update: OperationUpdate) : CoreEvent
    data class Log(val record: LogRecord) : CoreEvent
    data class UsbPairing(val event: PairEvent) : CoreEvent
}
