package io.github.souitouiop.iloader.core

import android.content.Context
import io.github.souitouiop.iloader.bridge.BridgeAppIds
import io.github.souitouiop.iloader.bridge.BridgeCertificate
import io.github.souitouiop.iloader.bridge.BridgeCore
import io.github.souitouiop.iloader.bridge.BridgeDevice
import io.github.souitouiop.iloader.bridge.BridgeErrorInfo
import io.github.souitouiop.iloader.bridge.BridgeEvent
import io.github.souitouiop.iloader.bridge.BridgeException
import io.github.souitouiop.iloader.bridge.BridgePairState
import io.github.souitouiop.iloader.bridge.BridgeUpdateType
import io.github.souitouiop.iloader.bridge.CoreListener
import io.github.souitouiop.iloader.platform.KeystoreSecretStore
import io.github.souitouiop.iloader.platform.UsbSink
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.withContext
import java.io.File

/** The Rust core (through the UniFFI bridge) behind the [IloaderCore] interface. */
class RealCore(context: Context) : IloaderCore, UsbSink {
    private val _events = MutableSharedFlow<CoreEvent>(
        extraBufferCapacity = 1024,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )
    override val events: SharedFlow<CoreEvent> = _events.asSharedFlow()

    private val bridge: BridgeCore

    init {
        val dataDir = File(context.filesDir, "core").apply { mkdirs() }
        val cacheDir = File(context.cacheDir, "core").apply { mkdirs() }
        bridge = BridgeCore(
            dataDir.path,
            cacheDir.path,
            KeystoreSecretStore(context),
            object : CoreListener {
                override fun onEvent(event: BridgeEvent) {
                    _events.tryEmit(event.toModel())
                }
            },
        )
    }

    // account
    override suspend fun loginNew(email: String, password: String, anisetteServer: String, saveCredentials: Boolean) =
        call { bridge.loginNew(email, password, anisetteServer, saveCredentials) }

    override suspend fun loginStored(email: String, anisetteServer: String) =
        call { bridge.loginStored(email, anisetteServer) }

    override fun loggedInAs(): String? = bridge.loggedInAs()
    override fun invalidateAccount() = bridge.invalidateAccount()
    override fun storedAccounts(): List<String> = bridge.storedAccounts()
    override fun deleteAccount(email: String) = call { bridge.deleteAccount(email) }
    override fun submitTwoFactor(code: String) = bridge.submitTwoFactor(code)
    override fun respondMaxCerts(serials: List<String>?) = bridge.respondMaxCerts(serials)

    override suspend fun certificates(): List<CertificateInfo> =
        call { bridge.certificates().map { it.toModel() } }

    override suspend fun revokeCertificate(serialNumber: String) = call { bridge.revokeCertificate(serialNumber) }
    override suspend fun listAppIds(): ListAppIdsResponse = call { bridge.listAppIds().toModel() }
    override suspend fun deleteAppId(appIdId: String) = call { bridge.deleteAppId(appIdId) }

    // devices
    override suspend fun listDevices(): List<DeviceResult> = call {
        bridge.listDevices().map { r ->
            val device = r.device
            val error = r.error
            when {
                device != null -> DeviceResult.Ok(device.toModel())
                error != null -> DeviceResult.Err(error.toModel())
                else -> DeviceResult.Err(IloaderException("misc", "Empty device entry"))
            }
        }
    }

    override suspend fun selectDevice(device: DeviceInfo?) = call { bridge.selectDevice(device?.toBridge()) }
    override fun cancelPairing() = bridge.cancelPairing()

    // operations
    override suspend fun sideload(appPath: String) = call { bridge.sideload(appPath) }
    override suspend fun installSidestore(nightly: Boolean, liveContainer: Boolean) =
        call { bridge.installSidestore(nightly, liveContainer) }

    // pairing files
    override suspend fun installedPairingApps(): List<PairingAppInfo> = call {
        bridge.installedPairingApps().map { PairingAppInfo(it.name, it.bundleId, it.path) }
    }

    override suspend fun placePairing(bundleId: String, path: String) = call { bridge.placePairing(bundleId, path) }
    override suspend fun exportPairing(): PairingExport = call {
        bridge.exportPairing().let { PairingExport(it.bytes, it.fileName) }
    }

    override suspend fun hasStoredRppairing(device: DeviceInfo): Boolean =
        call { bridge.hasStoredRppairing(device.toBridge()) }

    override suspend fun deleteStoredRppairing() = call { bridge.deleteStoredRppairing() }

    // settings related
    override fun keyringAvailable(): Boolean = bridge.keyringAvailable()
    override fun forceDisableKeyring(force: Boolean) = bridge.forceDisableKeyring(force)
    override suspend fun resetAnisetteState(): Boolean =
        withContext(Dispatchers.IO) { call { bridge.resetAnisetteState() } }

    // USB host
    override suspend fun attachUsbDevice(
        fd: Int,
        serial: String,
        productId: Int,
        epIn: Int,
        epOut: Int,
        maxPacketOut: Int,
        name: String,
    ) = call { bridge.attachUsbDevice(fd, serial, productId, epIn, epOut, maxPacketOut, name) }

    override fun detachUsbDevice(name: String) = bridge.detachUsbDevice(name)
}

private inline fun <T> call(block: () -> T): T = try {
    block()
} catch (e: BridgeException.App) {
    throw IloaderException(e.kind, e.detail)
}

private fun BridgeErrorInfo.toModel() = IloaderException(kind, detail)

private fun BridgeDevice.toModel() = DeviceInfo(name, id, udid, connectionType, version)
private fun DeviceInfo.toBridge() = BridgeDevice(name, id, udid, connectionType, version)

private fun BridgeCertificate.toModel() =
    CertificateInfo(name, certificateId, serialNumber, machineName, machineId)

private fun BridgeAppIds.toModel() = ListAppIdsResponse(
    appIds = appIds.map { AppIdInfo(it.appIdId, it.identifier, it.name, it.expirationDate) },
    maxQuantity = maxQuantity.toInt(),
    availableQuantity = availableQuantity.toInt(),
)

private fun BridgeEvent.toModel(): CoreEvent = when (this) {
    is BridgeEvent.TwoFactorRequired -> CoreEvent.TwoFactorRequired(TwoFactorParams())
    is BridgeEvent.MaxCertsReached -> CoreEvent.MaxCertsReached(certificates.map { it.toModel() })
    is BridgeEvent.Operation -> CoreEvent.Operation(
        operationId,
        OperationUpdate(
            when (updateType) {
                BridgeUpdateType.STARTED -> UpdateType.Started
                BridgeUpdateType.FINISHED -> UpdateType.Finished
                BridgeUpdateType.FAILED -> UpdateType.Failed
            },
            stepId,
            error?.toModel(),
        ),
    )
    is BridgeEvent.Log -> CoreEvent.Log(LogRecord(level, message, target, timestamp))
    is BridgeEvent.UsbPairing -> CoreEvent.UsbPairing(
        PairEvent(
            udid,
            when (val s = state) {
                is BridgePairState.Pairing -> PairState.Pairing
                is BridgePairState.WaitingForTrust -> PairState.WaitingForTrust
                is BridgePairState.Paired -> PairState.Paired
                is BridgePairState.Denied -> PairState.Denied
                is BridgePairState.Failed -> PairState.Failed(s.message)
            },
        ),
    )
}
