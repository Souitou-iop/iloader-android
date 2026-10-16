package io.github.souitouiop.iloader.ui

import android.app.Application
import android.net.Uri
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.souitouiop.iloader.IloaderApp
import io.github.souitouiop.iloader.R
import io.github.souitouiop.iloader.core.CertificateInfo
import io.github.souitouiop.iloader.core.CoreEvent
import io.github.souitouiop.iloader.core.DeviceInfo
import io.github.souitouiop.iloader.core.DeviceResult
import io.github.souitouiop.iloader.core.IloaderException
import io.github.souitouiop.iloader.core.ListAppIdsResponse
import io.github.souitouiop.iloader.core.LogRecord
import io.github.souitouiop.iloader.core.PairState
import io.github.souitouiop.iloader.core.PairingAppInfo
import io.github.souitouiop.iloader.core.PairingExport
import io.github.souitouiop.iloader.core.UpdateType
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

sealed interface Route {
    data object Home : Route
    data object Operation : Route
    data object Settings : Route
    data object Logs : Route
    data object Certificates : Route
    data object AppIds : Route
    data object Pairing : Route
}

class ErrorState(val title: UiText, val error: IloaderException)

class MaxCertsState(val certificates: List<CertificateInfo>)

/** Single source of UI state. All backend access goes through [IloaderCore][io.github.souitouiop.iloader.core.IloaderCore]. */
class AppViewModel(private val app: Application) : AndroidViewModel(app) {
    private val container = app as IloaderApp
    private val core = container.core
    val prefs = container.prefs

    // navigation
    val backStack = mutableStateListOf<Route>(Route.Home)
    val route: Route get() = backStack.last()

    fun navigate(target: Route) {
        if (route != target) backStack.add(target)
    }

    /** Returns false when already at the root. The operation page can only be left once it is done. */
    fun back(): Boolean {
        if (route == Route.Operation && operation?.done != true) return true
        if (backStack.size <= 1) return false
        backStack.removeAt(backStack.lastIndex)
        return true
    }

    // transient feedback
    var status by mutableStateOf<UiText?>(null)
        private set
    private val _messages = MutableSharedFlow<UiText>(extraBufferCapacity = 8)
    val messages: SharedFlow<UiText> = _messages.asSharedFlow()
    var error by mutableStateOf<ErrorState?>(null)
        private set

    fun toast(text: UiText) {
        _messages.tryEmit(text)
    }

    fun showError(title: UiText, e: Throwable) {
        error = ErrorState(title, e.asIloader())
    }

    fun dismissError() {
        error = null
    }

    private fun Throwable.asIloader(): IloaderException =
        this as? IloaderException ?: IloaderException("misc", message ?: toString())

    /** Runs [block] with a "loading" status and reports success or the error dialog, like `toast.promise`. */
    private fun action(
        loading: UiText,
        success: UiText?,
        failure: UiText,
        block: suspend () -> Unit,
    ): Job = viewModelScope.launch {
        status = loading
        try {
            block()
            success?.let(::toast)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            showError(failure, e)
        } finally {
            status = null
        }
    }

    // account
    var loggedInAs by mutableStateOf<String?>(null)
        private set
    var storedIds by mutableStateOf<List<String>>(emptyList())
        private set
    var keyringAvailable by mutableStateOf(true)
        private set
    var twoFactorOpen by mutableStateOf(false)
        private set
    var maxCerts by mutableStateOf<MaxCertsState?>(null)
        private set

    private fun refreshAccount() {
        loggedInAs = core.loggedInAs()
        storedIds = core.storedAccounts()
    }

    fun login(email: String, password: String, save: Boolean) {
        if (email.isBlank() || password.isEmpty()) {
            toast(UiText(R.string.apple_id_enter_email_password))
            return
        }
        action(UiText(R.string.apple_id_logging_in), UiText(R.string.apple_id_logged_in_success), UiText(R.string.apple_id_login_failed)) {
            try {
                core.loginNew(email.trim(), password, prefs.anisetteServer.first(), save)
            } finally {
                twoFactorOpen = false
                maxCerts = null
                refreshAccount()
            }
        }
    }

    fun loginStored(email: String) {
        action(UiText(R.string.apple_id_logging_in), UiText(R.string.apple_id_logged_in_success), UiText(R.string.apple_id_login_failed)) {
            try {
                core.loginStored(email, prefs.anisetteServer.first())
            } finally {
                twoFactorOpen = false
                refreshAccount()
            }
        }
    }

    fun signOut() {
        action(UiText(R.string.apple_id_signing_out), UiText(R.string.apple_id_signed_out_success), UiText(R.string.apple_id_sign_out_failed)) {
            core.invalidateAccount()
            refreshAccount()
        }
    }

    fun deleteAccount(email: String) {
        action(UiText(R.string.apple_id_deleting), UiText(R.string.apple_id_deleted_success), UiText(R.string.apple_id_deletion_failed)) {
            core.deleteAccount(email)
            refreshAccount()
        }
    }

    fun submitTwoFactor(code: String): Boolean {
        if (code.length != 6 || !code.all(Char::isDigit)) {
            toast(UiText(R.string.apple_id_valid_6digit))
            return false
        }
        core.submitTwoFactor(code)
        twoFactorOpen = false
        return true
    }

    fun respondMaxCerts(serials: List<String>?) {
        core.respondMaxCerts(serials?.takeIf { it.isNotEmpty() })
        maxCerts = null
    }

    // devices
    var devices by mutableStateOf<List<DeviceInfo>>(emptyList())
        private set
    var selectedDevice by mutableStateOf<DeviceInfo?>(null)
        private set
    var devicesLoading by mutableStateOf(false)
        private set

    /** The device we are waiting on, once the pairing dialog is due (after a short delay like on desktop). */
    var pairingDevice by mutableStateOf<DeviceInfo?>(null)
        private set
    var pairState by mutableStateOf<PairState>(PairState.Pairing)
        private set
    private var pairingRequest = 0
    private var pairingJob: Job? = null
    private var pairingDialogJob: Job? = null
    private var waitingToPair by mutableStateOf<DeviceInfo?>(null)
    val selecting get() = waitingToPair != null

    fun loadDevices() {
        if (devicesLoading) return
        devicesLoading = true
        viewModelScope.launch {
            status = UiText(R.string.device_loading_devices)
            try {
                val results = core.listDevices()
                val list = results.filterIsInstance<DeviceResult.Ok>().map { it.device }
                results.filterIsInstance<DeviceResult.Err>().forEach {
                    showError(UiText(R.string.device_unable_load_devices_prefix), it.error)
                }
                devices = list
                if (selectedDevice != null && list.none { it.id == selectedDevice?.id }) selectDevice(null)
                if (list.isNotEmpty()) {
                    val paired = list.firstOrNull { runCatching { core.hasStoredRppairing(it) }.getOrDefault(false) }
                    if (paired != null) selectDevice(paired)
                }
                toast(
                    UiText(
                        when {
                            list.isEmpty() -> R.string.device_no_devices_found
                            list.size > 1 -> R.string.device_found_devices
                            else -> R.string.device_found_device
                        },
                    ),
                )
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                devices = emptyList()
                selectDevice(null)
                showError(UiText(R.string.device_unable_load_devices_prefix), e)
            } finally {
                devicesLoading = false
                status = null
            }
        }
    }

    fun selectDevice(device: DeviceInfo?) {
        val request = ++pairingRequest
        pairingDialogJob?.cancel()
        pairingDevice = null
        waitingToPair = device
        pairState = PairState.Pairing
        if (device != null) {
            pairingDialogJob = viewModelScope.launch {
                delay(100)
                if (pairingRequest == request) pairingDevice = device
            }
        }
        pairingJob = viewModelScope.launch {
            try {
                core.selectDevice(device)
                if (pairingRequest != request) return@launch
                selectedDevice = device
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (pairingRequest != request) return@launch
                if (e.message != "Pairing cancelled") showError(UiText(R.string.device_failed_select), e)
            } finally {
                if (pairingRequest == request) {
                    pairingDialogJob?.cancel()
                    pairingDevice = null
                    waitingToPair = null
                }
            }
        }
    }

    fun cancelPairing() {
        pairingRequest++
        pairingDialogJob?.cancel()
        pairingDevice = null
        waitingToPair = null
        core.cancelPairing()
    }

    // operations
    var operation by mutableStateOf<OperationState?>(null)
        private set

    private fun ensureReady(needDevice: Boolean = true): Boolean {
        if (loggedInAs == null) {
            toast(UiText(R.string.app_must_be_logged_in))
            return false
        }
        if (needDevice && selectedDevice == null) {
            toast(UiText(R.string.app_must_select_device))
            return false
        }
        return true
    }

    fun requireLoginAndDevice(): Boolean = ensureReady()

    fun installSidestore(nightly: Boolean, liveContainer: Boolean) {
        if (!ensureReady()) return
        startOperation(if (liveContainer) OperationKind.LiveContainer else OperationKind.SideStore) {
            core.installSidestore(nightly, liveContainer)
        }
    }

    /** Copies the picked IPA into the cache directory and sideloads it. */
    fun sideloadIpa(uri: Uri) {
        if (!ensureReady()) return
        startOperation(OperationKind.Sideload) {
            val file = withContext(Dispatchers.IO) {
                val dir = File(app.cacheDir, "ipa").apply { mkdirs() }
                dir.listFiles()?.forEach { it.delete() }
                File(dir, "import.ipa").also { target ->
                    val input = app.contentResolver.openInputStream(uri)
                        ?: throw IloaderException("filesystem", "Cannot open the selected file")
                    input.use { src -> target.outputStream().use { src.copyTo(it) } }
                }
            }
            core.sideload(file.absolutePath)
        }
    }

    private fun startOperation(kind: OperationKind, block: suspend () -> Unit) {
        operation = OperationState(kind)
        navigate(Route.Operation)
        viewModelScope.launch {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // Progress events carry the failure; add it if the core failed before emitting one.
                val current = operation
                if (current != null && !current.hasFailed) {
                    val step = current.kind.steps.firstOrNull { it.id !in current.completed }?.id ?: current.kind.steps.first().id
                    operation = current.copy(
                        started = current.started + step,
                        failed = current.failed + StepFailure(step, e.asIloader()),
                    )
                }
            }
        }
    }

    fun closeOperation() {
        operation = null
        if (route == Route.Operation) backStack.removeAt(backStack.lastIndex)
    }

    private fun onOperationUpdate(operationId: String, update: io.github.souitouiop.iloader.core.OperationUpdate) {
        val old = operation ?: return
        if (old.kind.id != operationId) return
        operation = when (update.updateType) {
            UpdateType.Started -> old.copy(started = old.started + update.stepId)
            UpdateType.Finished -> old.copy(completed = old.completed + update.stepId)
            UpdateType.Failed -> old.copy(
                failed = old.failed + StepFailure(
                    update.stepId,
                    update.extraDetails ?: IloaderException("misc", ""),
                ),
            )
        }
    }

    // certificates
    var certificates by mutableStateOf<List<CertificateInfo>>(emptyList())
        private set
    var certificatesLoading by mutableStateOf(false)
        private set

    fun loadCertificates() {
        if (certificatesLoading) return
        certificatesLoading = true
        action(UiText(R.string.certificates_loading), null, UiText(R.string.certificates_failed_load)) {
            try {
                certificates = core.certificates()
            } finally {
                certificatesLoading = false
            }
        }
    }

    fun revokeCertificate(serial: String) {
        action(UiText(R.string.certificates_revoking), UiText(R.string.certificates_revoked_success), UiText(R.string.certificates_failed_revoke)) {
            core.revokeCertificate(serial)
            certificates = core.certificates()
        }
    }

    // app ids
    var appIds by mutableStateOf<ListAppIdsResponse?>(null)
        private set
    var appIdsLoading by mutableStateOf(false)
        private set

    fun loadAppIds() {
        if (appIdsLoading) return
        appIdsLoading = true
        action(UiText(R.string.app_ids_loading), null, UiText(R.string.app_ids_failed_load)) {
            try {
                appIds = core.listAppIds()
            } finally {
                appIdsLoading = false
            }
        }
    }

    fun deleteAppId(id: String) {
        action(UiText(R.string.apple_id_deleting), UiText(R.string.app_ids_deleted_success), UiText(R.string.app_ids_failed_delete)) {
            core.deleteAppId(id)
            appIds = core.listAppIds()
        }
    }

    // pairing files
    var pairingApps by mutableStateOf<List<PairingAppInfo>>(emptyList())
        private set
    var pairingAppsLoading by mutableStateOf(false)
        private set
    var pairingAppsFailed by mutableStateOf(false)
        private set
    private val _exports = MutableSharedFlow<PairingExport>(extraBufferCapacity = 1)

    /** Emits once the core produced the pairing file; the UI asks where to save it. */
    val exports: SharedFlow<PairingExport> = _exports.asSharedFlow()

    fun loadPairingApps() {
        if (pairingAppsLoading) return
        pairingAppsLoading = true
        action(UiText(R.string.pairing_loading_apps), null, UiText(R.string.pairing_failed_load_apps)) {
            try {
                pairingApps = core.installedPairingApps()
                pairingAppsFailed = false
            } catch (e: Exception) {
                pairingAppsFailed = true
                throw e
            } finally {
                pairingAppsLoading = false
            }
        }
    }

    fun placePairing(apps: List<PairingAppInfo>) {
        action(UiText(R.string.pairing_placing_pairing_file), UiText(R.string.pairing_pairing_file_placed_success), UiText(R.string.pairing_failed_place_pairing)) {
            for (a in apps) core.placePairing(a.bundleId, a.path)
        }
    }

    fun requestExport() {
        action(UiText(R.string.pairing_exporting_pairing_file), null, UiText(R.string.pairing_failed_export_pairing_file)) {
            _exports.emit(core.exportPairing())
        }
    }

    fun saveExport(uri: Uri, export: PairingExport) {
        action(UiText(R.string.pairing_exporting_pairing_file), UiText(R.string.pairing_pairing_file_exported_success), UiText(R.string.pairing_failed_export_pairing_file)) {
            withContext(Dispatchers.IO) {
                val out = app.contentResolver.openOutputStream(uri)
                    ?: throw IloaderException("filesystem", "Cannot write to the selected location")
                out.use { it.write(export.bytes) }
            }
        }
    }

    fun openPairing() {
        if (selectedDevice == null) toast(UiText(R.string.app_must_select_device)) else navigate(Route.Pairing)
    }

    fun openCertificates() {
        if (ensureReady(needDevice = false)) navigate(Route.Certificates)
    }

    fun openAppIds() {
        if (ensureReady(needDevice = false)) navigate(Route.AppIds)
    }

    // settings
    val logs = mutableStateListOf<LogRecord>()
    var overrideKeyring by mutableStateOf(false)
        private set

    fun setAnisetteServer(value: String) {
        viewModelScope.launch { prefs.setAnisetteServer(value.trim().ifEmpty { io.github.souitouiop.iloader.data.Prefs.DEFAULT_ANISETTE }) }
    }

    fun setLang(value: String) {
        viewModelScope.launch { prefs.setLang(value) }
    }

    fun changeOverrideKeyring(value: Boolean) {
        overrideKeyring = value
        viewModelScope.launch {
            prefs.setOverrideKeyring(value)
            core.forceDisableKeyring(value)
            keyringAvailable = core.keyringAvailable()
        }
    }

    fun resetAnisette() {
        action(UiText(R.string.settings_resetting_anisette_state), null, UiText(R.string.settings_failed_reset_anisette_state)) {
            val didReset = core.resetAnisetteState()
            toast(UiText(if (didReset) R.string.settings_anisette_state_reset_success else R.string.settings_anisette_state_not_found))
        }
    }

    fun deleteStoredPairing() {
        if (selectedDevice == null) {
            toast(UiText(R.string.app_must_select_device))
            return
        }
        action(UiText(R.string.settings_deleting_stored_rppairing), UiText(R.string.settings_stored_rppairing_deleted_success), UiText(R.string.settings_failed_delete_stored_rppairing)) {
            core.deleteStoredRppairing()
            core.selectDevice(null)
            selectedDevice = null
        }
    }

    init {
        viewModelScope.launch {
            overrideKeyring = prefs.overrideKeyring.first()
            core.forceDisableKeyring(overrideKeyring)
            keyringAvailable = core.keyringAvailable()
        }
        refreshAccount()
        viewModelScope.launch {
            core.events.collect { event ->
                when (event) {
                    is CoreEvent.TwoFactorRequired -> twoFactorOpen = true
                    is CoreEvent.MaxCertsReached -> maxCerts = MaxCertsState(event.certificates)
                    is CoreEvent.Operation -> onOperationUpdate(event.operationId, event.update)
                    is CoreEvent.Log -> {
                        logs.add(event.record)
                        if (logs.size > MAX_LOGS) logs.removeRange(0, logs.size - MAX_LOGS)
                    }
                    is CoreEvent.UsbPairing -> if (event.event.udid == pairingDevice?.udid || event.event.udid == waitingToPair?.udid) {
                        pairState = event.event.state
                    }
                }
            }
        }
        loadDevices()
    }

    private companion object {
        const val MAX_LOGS = 2000
    }
}
