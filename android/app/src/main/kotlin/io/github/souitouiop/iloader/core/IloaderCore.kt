package io.github.souitouiop.iloader.core

import kotlinx.coroutines.flow.SharedFlow

/**
 * Everything the UI needs from the backend. The UI only knows this interface,
 * so the Rust-backed implementation can replace [FakeCore] without UI changes.
 * Failures are thrown as [IloaderException].
 */
interface IloaderCore {
    val events: SharedFlow<CoreEvent>

    // account
    suspend fun loginNew(email: String, password: String, anisetteServer: String, saveCredentials: Boolean)
    suspend fun loginStored(email: String, anisetteServer: String)
    fun loggedInAs(): String?
    fun invalidateAccount()
    fun storedAccounts(): List<String>
    fun deleteAccount(email: String)
    fun submitTwoFactor(code: String)
    fun respondMaxCerts(serials: List<String>?)
    suspend fun certificates(): List<CertificateInfo>
    suspend fun revokeCertificate(serialNumber: String)
    suspend fun listAppIds(): ListAppIdsResponse
    suspend fun deleteAppId(appIdId: String)

    // devices
    suspend fun listDevices(): List<DeviceResult>
    suspend fun selectDevice(device: DeviceInfo?)
    fun cancelPairing()

    // operations, progress arrives as CoreEvent.Operation
    suspend fun sideload(appPath: String)
    suspend fun installSidestore(nightly: Boolean, liveContainer: Boolean)

    // pairing files
    suspend fun installedPairingApps(): List<PairingAppInfo>
    suspend fun placePairing(bundleId: String, path: String)
    suspend fun exportPairing(): PairingExport
    suspend fun hasStoredRppairing(device: DeviceInfo): Boolean
    suspend fun deleteStoredRppairing()

    // settings related
    fun keyringAvailable(): Boolean
    fun forceDisableKeyring(force: Boolean)
    suspend fun resetAnisetteState(): Boolean
}

object OperationIds {
    const val SIDELOAD = "sideload"
    const val INSTALL_SIDESTORE = "install_sidestore"
}
