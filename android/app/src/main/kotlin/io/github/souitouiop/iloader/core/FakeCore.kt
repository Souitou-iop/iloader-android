package io.github.souitouiop.iloader.core

import java.time.Instant
import java.time.temporal.ChronoUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.withTimeoutOrNull

/**
 * In-memory [IloaderCore] with believable data and timings, so the whole UI can
 * be exercised without the Rust backend.
 *
 * Behaviour can be steered from the Apple ID email, so every path is reachable
 * on a device. Put one of these words anywhere in the address:
 *
 * login:      `badauth`, `locked`, `underage`, `anisette`, `keyring`, `certs`
 *             (too many certificates), `no2fa` (skip the 2FA prompt)
 * operations: `download`, `maxapps`, `noappids`, `house`, `pairing`,
 *             `usbmuxd`, `devicecoms`, `canceled`, `storage`, `misc`, `fs`
 *             (fail the matching step of install/sideload)
 *
 * Without a keyword everything succeeds. The 2FA code `000000` is rejected.
 * Device refreshes: every third call also returns a device error.
 */
class FakeCore(
    /** Scales every delay; tests use 0. */
    private val speed: Double = 1.0,
) : IloaderCore {
    private val _events = MutableSharedFlow<CoreEvent>(extraBufferCapacity = 256)
    override val events: SharedFlow<CoreEvent> = _events.asSharedFlow()

    private var account: String? = null
    private val saved = mutableListOf("demo@icloud.com")
    private var twoFactor: CompletableDeferred<String?>? = null
    private var maxCerts: CompletableDeferred<List<String>?>? = null
    private var pairingCancel: CompletableDeferred<Unit>? = null
    private val pairedUdids = mutableSetOf<String>()
    private var selected: DeviceInfo? = null
    private val listCalls = AtomicInteger()
    private var anisetteResets = 0

    private val certs = mutableListOf(
        CertificateInfo("iOS Development: Alex Doe", "C8Q2X4K9LM", "5A3F0C21D7E94B6A", "iloader", "9D1C3E55-7B1A-4F0E-8E3B-2A6C4F7D9B10"),
        CertificateInfo("iOS Development: Alex Doe", "T7N5V1B3QP", "1E8B72AA40C5D93F", "AltServer on Alex-MacBook", "4F2A6B90-31C8-4D7E-A5B2-0C9E8D1F3A67"),
        CertificateInfo("iOS Development: Alex Doe", "H2J6W8Z0RS", "7C40D9E1B2A35F68", "SideStore", "B3E7A1C4-5D92-48F0-9A16-7E2C0D4B8F35"),
    )
    private val appIds = mutableListOf(
        AppIdInfo("Y7K2M9PQ4T", "com.SideStore.SideStore.A1B2C3D4E5", "SideStore", daysFromNow(6)),
        AppIdInfo("L3N8R5VX1C", "com.SideStore.SideStore.A1B2C3D4E5.LiveActivity", "SideStore Widget", daysFromNow(6)),
        AppIdInfo("D9F4G6HJ2K", "com.kdt.livecontainer.A1B2C3D4E5", "LiveContainer", daysFromNow(3)),
        AppIdInfo("B5M1Q7WZ8X", "me.example.Sample.A1B2C3D4E5", "Sample", daysFromNow(1)),
        AppIdInfo("R2T6Y0UI3O", "com.example.Notes.A1B2C3D4E5", "Notes+", null),
    )
    private val pairingApps = listOf(
        PairingAppInfo("SideStore", "com.SideStore.SideStore", "ALTPairingFile.mobiledevicepairing"),
        PairingAppInfo("LiveContainer", "com.kdt.livecontainer", "SideStore/Documents/ALTPairingFile.mobiledevicepairing"),
        PairingAppInfo("StikDebug", "com.stik.stikebug", "pairingFile.plist"),
        PairingAppInfo("Reynard", "app.reynard.browser", "pairingFile.plist"),
    )
    private val devices = listOf(
        DeviceInfo("Alex's iPhone", 3, "00008130-001A2B3C4D5E001E", "USB", "18.2"),
        DeviceInfo("iPad Pro", 5, "00008112-000C4D5E6F7A801E", "Network", "17.6.1"),
    )

    private fun daysFromNow(days: Long) = Instant.now().plus(days, ChronoUnit.DAYS).toString()

    private suspend fun pause(millis: Long) = delay((millis * speed).toLong())

    private fun log(level: Int, target: String, message: String) {
        _events.tryEmit(CoreEvent.Log(LogRecord(level, message, target, Instant.now().toString())))
    }

    private fun failure(type: String, message: String) = IloaderException(type, message)

    // account

    override suspend fun loginNew(email: String, password: String, anisetteServer: String, saveCredentials: Boolean) {
        log(LogRecord.INFO, "iloader::account", "Logging in as $email via $anisetteServer")
        pause(900)
        val tag = email.lowercase()
        when {
            "anisette" in tag -> throw failure("anisette", "Failed to fetch anisette data from $anisetteServer: connection timed out")
            "keyring" in tag && saveCredentials ->
                throw failure("keyring_with_message", "Keyring error: Failed to store credentials - Secret Service is not available")
        }
        if ("no2fa" !in tag) {
            val code = awaitTwoFactor() ?: throw failure("auth", "Timed out waiting for the 2FA code")
            if (code == "000000") {
                log(LogRecord.WARN, "iloader::account", "2FA code rejected")
                throw failure("auth", "Login failed\n● Incorrect verification code (-22406)")
            }
            pause(800)
        }
        when {
            "badauth" in tag || password == "wrong" ->
                throw failure("auth", "Login failed\n● Your Apple ID or password was incorrect. (-20101)")
            "locked" in tag -> throw failure("account_locked", "Login failed\n● This Apple ID has been locked for security reasons. (-20209)")
            "underage" in tag -> throw failure("underage", "Developer error\n● You must be 18 or older to register as a developer. (1102)")
        }
        if ("certs" in tag) resolveMaxCerts()
        account = email
        if (saveCredentials && email !in saved) saved += email
        log(LogRecord.INFO, "iloader::account", "Logged in as $email")
    }

    override suspend fun loginStored(email: String, anisetteServer: String) {
        log(LogRecord.INFO, "iloader::account", "Logging in with stored credentials for $email")
        pause(1200)
        if ("anisette" in email.lowercase()) {
            throw failure("anisette", "Failed to fetch anisette data from $anisetteServer: connection timed out")
        }
        account = email
        log(LogRecord.INFO, "iloader::account", "Logged in as $email")
    }

    private suspend fun awaitTwoFactor(): String? {
        val answer = CompletableDeferred<String?>()
        twoFactor = answer
        _events.tryEmit(CoreEvent.TwoFactorRequired(TwoFactorParams()))
        val code = withTimeoutOrNull((120_000 * speed).toLong().coerceAtLeast(1)) { answer.await() }
        twoFactor = null
        return code
    }

    private suspend fun resolveMaxCerts() {
        val answer = CompletableDeferred<List<String>?>()
        maxCerts = answer
        _events.tryEmit(CoreEvent.MaxCertsReached(certs.toList()))
        val serials = withTimeoutOrNull((300_000 * speed).toLong().coerceAtLeast(1)) { answer.await() }
        maxCerts = null
        if (serials == null) throw failure("canceled", "Certificate revocation canceled")
        pause(700)
        certs.removeAll { it.serialNumber in serials }
        log(LogRecord.INFO, "iloader::account", "Revoked ${serials.size} certificate(s)")
    }

    override fun loggedInAs(): String? = account

    override fun invalidateAccount() {
        account = null
    }

    override fun storedAccounts(): List<String> = saved.toList()

    override fun deleteAccount(email: String) {
        saved.remove(email)
    }

    override fun submitTwoFactor(code: String) {
        twoFactor?.complete(code)
    }

    override fun respondMaxCerts(serials: List<String>?) {
        maxCerts?.complete(serials)
    }

    override suspend fun certificates(): List<CertificateInfo> {
        requireLogin()
        pause(800)
        return certs.toList()
    }

    override suspend fun revokeCertificate(serialNumber: String) {
        requireLogin()
        pause(900)
        if (!certs.removeAll { it.serialNumber == serialNumber }) {
            throw failure("developer", "Developer error\n● Certificate not found (7252)")
        }
        log(LogRecord.INFO, "iloader::account", "Revoked certificate $serialNumber")
    }

    override suspend fun listAppIds(): ListAppIdsResponse {
        requireLogin()
        pause(800)
        return ListAppIdsResponse(appIds.toList(), maxQuantity = 10, availableQuantity = 10 - appIds.size)
    }

    override suspend fun deleteAppId(appIdId: String) {
        requireLogin()
        pause(700)
        appIds.removeAll { it.appIdId == appIdId }
    }

    private fun requireLogin() {
        if (account == null) throw failure("not_logged_in", "Not logged in")
    }

    // devices

    override suspend fun listDevices(): List<DeviceResult> {
        pause(1000)
        log(LogRecord.INFO, "iloader::device", "Listing devices")
        val call = listCalls.incrementAndGet()
        val results = devices.map<DeviceInfo, DeviceResult> { DeviceResult.Ok(it) }.toMutableList()
        if (call % 3 == 0) {
            results += DeviceResult.Err(failure("device_coms_with_message", "Failed to read device info: lockdownd returned InvalidHostID"))
        }
        return results
    }

    override suspend fun selectDevice(device: DeviceInfo?) {
        pairingCancel?.complete(Unit)
        if (device == null) {
            selected = null
            return
        }
        if (device.udid in pairedUdids) {
            pause(400)
            selected = device
            return
        }
        val cancel = CompletableDeferred<Unit>()
        pairingCancel = cancel
        try {
            pairingEvent(device, PairState.Pairing)
            if (waitOrCancel(1200, cancel)) throw pairingCancelled()
            pairingEvent(device, PairState.WaitingForTrust)
            if (waitOrCancel(4500, cancel)) throw pairingCancelled()
            pairedUdids += device.udid
            pairingEvent(device, PairState.Paired)
            log(LogRecord.INFO, "iloader::device", "Paired with ${device.name}")
            selected = device
        } finally {
            if (pairingCancel === cancel) pairingCancel = null
        }
    }

    private fun pairingEvent(device: DeviceInfo, state: PairState) {
        _events.tryEmit(CoreEvent.UsbPairing(PairEvent(device.udid, state)))
    }

    private fun pairingCancelled() = failure("canceled", "Pairing cancelled")

    /** Waits [millis]; returns true when [cancel] completed first. */
    private suspend fun waitOrCancel(millis: Long, cancel: CompletableDeferred<Unit>): Boolean =
        withTimeoutOrNull((millis * speed).toLong().coerceAtLeast(1)) { cancel.await() } != null

    override fun cancelPairing() {
        pairingCancel?.complete(Unit)
    }

    // operations

    override suspend fun sideload(appPath: String) {
        requireLogin()
        val device = selected ?: throw failure("no_device_selected", "No device selected")
        log(LogRecord.INFO, "iloader::sideload", "Sideloading $appPath to ${device.name}")
        runSteps(OperationIds.SIDELOAD, listOf("install" to 4000L))
    }

    override suspend fun installSidestore(nightly: Boolean, liveContainer: Boolean) {
        requireLogin()
        val device = selected ?: throw failure("no_device_selected", "No device selected")
        val name = if (liveContainer) "LiveContainer+SideStore" else "SideStore"
        log(LogRecord.INFO, "iloader::sideload", "Installing $name (${if (nightly) "nightly" else "stable"}) to ${device.name}")
        runSteps(
            OperationIds.INSTALL_SIDESTORE,
            listOf("download" to 2200L, "install" to 4500L, "pairing" to 1500L),
        )
    }

    private suspend fun runSteps(operationId: String, steps: List<Pair<String, Long>>) {
        val email = account.orEmpty().lowercase()
        for ((step, millis) in steps) {
            emitOperation(operationId, UpdateType.Started, step)
            pause(millis)
            val error = failureFor(email, step)
            if (error != null) {
                log(LogRecord.ERROR, "iloader::sideload", "Step $step failed: ${error.message}")
                emitOperation(operationId, UpdateType.Failed, step, error)
                throw error
            }
            emitOperation(operationId, UpdateType.Finished, step)
        }
    }

    private fun emitOperation(id: String, type: UpdateType, step: String, error: IloaderException? = null) {
        _events.tryEmit(CoreEvent.Operation(id, OperationUpdate(type, step, error)))
    }

    private fun failureFor(email: String, step: String): IloaderException? = when {
        step == "download" && "download" in email ->
            failure("download", "Failed to download SideStore\n● error sending request for url (https://github.com/SideStore/SideStore/releases/download/0.6.2/SideStore.ipa)")
        step == "install" -> when {
            "maxapps" in email -> failure("max_apps", "Developer error\n● You have reached the maximum number of apps (3) for a free account.")
            "noappids" in email -> failure("not_enough_app_ids", "You need 2 App IDs but only 1 is available. Try again after an existing App ID expires.")
            "house" in email -> failure("house_arrest", "Failed to open house arrest service: InvalidService")
            "usbmuxd" in email -> failure("usbmuxd", "Failed to connect to usbmuxd: Connection refused (os error 111)")
            "devicecoms" in email -> failure("device_coms_with_message", "Failed to install app: AFC returned an error (-402620395)")
            "canceled" in email -> failure("canceled", "Install canceled")
            "storage" in email -> failure("storage", "Failed to save certificate: Permission denied")
            "misc" in email -> failure("misc", "Something unexpected happened")
            "fs" in email -> failure("filesystem", "Failed to read the IPA: No such file or directory (os error 2)")
            "anisette" in email -> failure("anisette", "Anisette server returned HTTP 502")
            else -> null
        }
        step == "pairing" && "pairing" in email ->
            failure("remote_pairing", "Failed to place the pairing file: SideStore is not installed")
        else -> null
    }

    // pairing files

    override suspend fun installedPairingApps(): List<PairingAppInfo> {
        if (selected == null) throw failure("no_device_selected", "No device selected")
        pause(1100)
        return pairingApps.take(3)
    }

    override suspend fun placePairing(bundleId: String, path: String) {
        pause(900)
        log(LogRecord.INFO, "iloader::pairing", "Placed pairing file in $bundleId at $path")
    }

    override suspend fun exportPairing(): PairingExport {
        val device = selected ?: throw failure("no_device_selected", "No device selected")
        pause(600)
        val plist = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>" +
            "<key>UDID</key><string>${device.udid}</string></dict></plist>\n"
        return PairingExport(plist.toByteArray(), "${device.udid}.mobiledevicepairing")
    }

    override suspend fun hasStoredRppairing(device: DeviceInfo): Boolean = device.udid in pairedUdids

    override suspend fun deleteStoredRppairing() {
        pause(600)
        val device = selected ?: throw failure("no_device_selected", "No device selected")
        pairedUdids -= device.udid
        selected = null
    }

    // settings related

    override fun keyringAvailable(): Boolean = true

    override fun forceDisableKeyring(force: Boolean) {
        log(LogRecord.INFO, "iloader::settings", "Keyring override set to $force")
    }

    override suspend fun resetAnisetteState(): Boolean {
        pause(700)
        return anisetteResets++ == 0
    }
}
