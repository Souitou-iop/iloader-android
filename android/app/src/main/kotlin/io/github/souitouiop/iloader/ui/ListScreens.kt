package io.github.souitouiop.iloader.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.github.souitouiop.iloader.R
import io.github.souitouiop.iloader.core.CertificateInfo
import io.github.souitouiop.iloader.core.AppIdInfo
import io.github.souitouiop.iloader.core.PairingExport
import java.time.OffsetDateTime
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import top.yukonga.miuix.kmp.basic.BasicComponent
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.SmallTitle
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
private fun EmptyHint(text: String) {
    Text(
        text,
        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
        modifier = Modifier.padding(horizontal = 28.dp, vertical = 16.dp),
    )
}

@Composable
private fun DetailLine(label: String, value: String?) {
    if (value.isNullOrEmpty()) return
    Text(
        text = "$label: $value",
        style = MiuixTheme.textStyles.footnote1,
        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
    )
}

@Composable
fun CertificatesScreen(vm: AppViewModel) {
    LaunchedEffect(Unit) { vm.loadCertificates() }
    var revoking by remember { mutableStateOf<CertificateInfo?>(null) }
    PageScaffold(title = stringResource(R.string.certificates_manage), onBack = { vm.back() }) {
        if (vm.certificates.isEmpty()) {
            item(key = "empty") {
                EmptyHint(
                    stringResource(if (vm.certificatesLoading) R.string.certificates_loading else R.string.certificates_none_found),
                )
            }
        } else {
            items(vm.certificates, key = { it.serialNumber ?: it.certificateId ?: it.hashCode() }) { cert ->
                Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                    BasicComponent(
                        title = cert.name.orEmpty(),
                        bottomAction = {
                            Column {
                                DetailLine(stringResource(R.string.certificates_serial_number), cert.serialNumber)
                                DetailLine(stringResource(R.string.certificates_machine_name), cert.machineName)
                                DetailLine(stringResource(R.string.certificates_machine_id), cert.machineId)
                            }
                        },
                        endActions = {
                            TextButton(
                                text = stringResource(R.string.certificates_revoke),
                                onClick = { revoking = cert },
                                minWidth = 0.dp,
                                colors = ButtonDefaults.textButtonColors(textColor = MiuixTheme.colorScheme.error),
                            )
                        },
                    )
                }
            }
        }
        item(key = "refresh") {
            TextButton(
                text = stringResource(R.string.common_refresh),
                onClick = vm::loadCertificates,
                enabled = !vm.certificatesLoading,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
            )
        }
    }
    ConfirmDialog(
        show = revoking != null,
        title = stringResource(R.string.certificates_revoke),
        message = stringResource(R.string.native_revoke_certificate_message, revoking?.name.orEmpty()),
        onConfirm = { revoking?.serialNumber?.let(vm::revokeCertificate) },
        onDismiss = { revoking = null },
        destructive = true,
    )
}

@Composable
fun AppIdsScreen(vm: AppViewModel) {
    LaunchedEffect(Unit) { vm.loadAppIds() }
    var deleting by remember { mutableStateOf<AppIdInfo?>(null) }
    val response = vm.appIds
    PageScaffold(title = stringResource(R.string.app_ids_manage), onBack = { vm.back() }) {
        if (response != null) {
            item(key = "quota") {
                SmallTitle(
                    stringResource(R.string.app_ids_available, response.availableQuantity.toString(), response.maxQuantity.toString()),
                )
            }
        }
        if (response == null || response.appIds.isEmpty()) {
            item(key = "empty") {
                EmptyHint(stringResource(if (vm.appIdsLoading) R.string.app_ids_loading else R.string.app_ids_none_found))
            }
        } else {
            items(response.appIds, key = { it.appIdId }) { appId ->
                Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                    BasicComponent(
                        title = appId.name,
                        bottomAction = {
                            Column {
                                DetailLine(stringResource(R.string.app_ids_identifier), appId.identifier)
                                DetailLine(stringResource(R.string.app_ids_id), appId.appIdId)
                                DetailLine(stringResource(R.string.app_ids_expiration), formatExpiration(appId.expirationDate))
                            }
                        },
                        endActions = {
                            TextButton(
                                text = stringResource(R.string.common_delete),
                                onClick = { deleting = appId },
                                minWidth = 0.dp,
                                colors = ButtonDefaults.textButtonColors(textColor = MiuixTheme.colorScheme.error),
                            )
                        },
                    )
                }
            }
        }
        item(key = "refresh") {
            TextButton(
                text = stringResource(R.string.common_refresh),
                onClick = vm::loadAppIds,
                enabled = !vm.appIdsLoading,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
            )
        }
    }
    ConfirmDialog(
        show = deleting != null,
        title = stringResource(R.string.common_delete),
        message = stringResource(R.string.native_delete_app_id_message, deleting?.name.orEmpty()),
        onConfirm = { deleting?.appIdId?.let(vm::deleteAppId) },
        onDismiss = { deleting = null },
        destructive = true,
    )
}

@Composable
private fun formatExpiration(iso: String?): String {
    if (iso == null) return stringResource(R.string.app_ids_never)
    return runCatching {
        OffsetDateTime.parse(iso).format(DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM))
    }.getOrDefault(iso)
}

@Composable
fun PairingScreen(vm: AppViewModel) {
    LaunchedEffect(Unit) { vm.loadPairingApps() }
    var confirmExport by remember { mutableStateOf(false) }
    var pendingExport by remember { mutableStateOf<PairingExport?>(null) }
    val saver = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri: Uri? ->
        val export = pendingExport
        pendingExport = null
        if (uri != null && export != null) vm.saveExport(uri, export)
    }
    LaunchedEffect(Unit) {
        vm.exports.collect {
            pendingExport = it
            saver.launch(it.fileName)
        }
    }
    PageScaffold(title = stringResource(R.string.pairing_manage), onBack = { vm.back() }) {
        if (vm.pairingApps.isEmpty()) {
            item(key = "empty") {
                EmptyHint(
                    stringResource(
                        when {
                            vm.pairingAppsLoading -> R.string.pairing_loading_app
                            vm.pairingAppsFailed -> R.string.pairing_failed_load_apps
                            else -> R.string.pairing_no_supported_apps_found
                        },
                    ),
                )
            }
        } else {
            item(key = "apps") {
                Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                    vm.pairingApps.forEach { app ->
                        BasicComponent(
                            title = app.name,
                            summary = app.bundleId,
                            endActions = {
                                TextButton(
                                    text = stringResource(R.string.pairing_place),
                                    onClick = { vm.placePairing(listOf(app)) },
                                    minWidth = 0.dp,
                                    colors = ButtonDefaults.textButtonColorsPrimary(),
                                )
                            },
                        )
                    }
                }
            }
            item(key = "place_all") {
                TextButton(
                    text = stringResource(R.string.pairing_place_all),
                    onClick = { vm.placePairing(vm.pairingApps) },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp).padding(bottom = 12.dp),
                    colors = ButtonDefaults.textButtonColorsPrimary(),
                )
            }
        }
        item(key = "actions") {
            Column(
                Modifier.padding(horizontal = 12.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                TextButton(
                    text = stringResource(R.string.pairing_refresh_installed_apps),
                    onClick = vm::loadPairingApps,
                    enabled = !vm.pairingAppsLoading,
                    modifier = Modifier.fillMaxWidth(),
                )
                TextButton(
                    text = stringResource(R.string.pairing_export_not_recommended),
                    onClick = { confirmExport = true },
                    modifier = Modifier.fillMaxWidth(),
                    colors = ButtonDefaults.textButtonColors(textColor = MiuixTheme.colorScheme.error),
                )
            }
        }
    }
    ConfirmDialog(
        show = confirmExport,
        title = stringResource(R.string.pairing_advanced_export_title),
        message = stringResource(R.string.pairing_advanced_export_message),
        onConfirm = vm::requestExport,
        onDismiss = { confirmExport = false },
        destructive = true,
    )
}
