package io.github.souitouiop.iloader.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.background
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import io.github.souitouiop.iloader.BuildConfig
import io.github.souitouiop.iloader.R
import io.github.souitouiop.iloader.core.DeviceInfo
import top.yukonga.miuix.kmp.basic.BasicComponent
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.SmallTitle
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Settings
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.preference.CheckboxPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme

private const val DONATE_URL = "https://github.com/sponsors/nab138"
private const val REPO_URL = "https://github.com/Souitou-iop/iloader-android"

@Composable
fun HomeScreen(vm: AppViewModel) {
    val context = LocalContext.current
    val ipaPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri != null) vm.sideloadIpa(uri)
    }
    PageScaffold(
        title = stringResource(R.string.native_app_name),
        subtitle = stringResource(R.string.subtitle),
        onBack = null,
        actions = {
            IconButton(onClick = { vm.navigate(Route.Settings) }) {
                Icon(
                    imageVector = MiuixIcons.Settings,
                    contentDescription = stringResource(R.string.app_settings),
                    tint = MiuixTheme.colorScheme.onBackground,
                )
            }
        },
    ) {
        item(key = "header") {
            HeaderCard(
                onDonate = { if (!context.openUrl(DONATE_URL)) vm.toast(UiText(R.string.app_open_github_failed)) },
                onGithub = { if (!context.openUrl(REPO_URL)) vm.toast(UiText(R.string.app_open_github_failed)) },
            )
        }
        item(key = "account") {
            SmallTitle(stringResource(R.string.app_section_account))
            Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) { AccountContent(vm) }
        }
        item(key = "device") {
            val hint = vm.selectedDevice?.let { stringResource(R.string.app_active_device, "${it.name} (${it.version})") }
                ?: stringResource(R.string.app_select_device)
            SmallTitle(stringResource(R.string.app_devices) + " · " + hint)
            Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) { DeviceContent(vm) }
        }
        item(key = "installers") {
            SmallTitle(stringResource(R.string.app_installers) + " · " + stringResource(R.string.app_choose_build))
            Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                ArrowPreference(
                    title = stringResource(R.string.app_sidestore_stable),
                    onClick = { vm.installSidestore(nightly = false, liveContainer = false) },
                )
                ArrowPreference(
                    title = stringResource(R.string.app_sidestore_nightly),
                    onClick = { vm.installSidestore(nightly = true, liveContainer = false) },
                )
                ArrowPreference(
                    title = stringResource(R.string.app_livecontainer_sidestore_stable),
                    onClick = { vm.installSidestore(nightly = false, liveContainer = true) },
                )
                ArrowPreference(
                    title = stringResource(R.string.app_livecontainer_sidestore_nightly),
                    onClick = { vm.installSidestore(nightly = true, liveContainer = true) },
                )
                ArrowPreference(
                    title = stringResource(R.string.app_import_ipa),
                    onClick = {
                        if (vm.requireLoginAndDevice()) {
                            ipaPicker.launch(arrayOf("application/octet-stream", "application/x-itunes-ipa", "application/zip", "*/*"))
                        }
                    },
                )
            }
        }
        item(key = "management") {
            SmallTitle(stringResource(R.string.app_section_management))
            Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                ArrowPreference(
                    title = stringResource(R.string.app_manage_pairing_file),
                    onClick = vm::openPairing,
                )
                ArrowPreference(
                    title = stringResource(R.string.app_refresh_devices),
                    onClick = vm::loadDevices,
                )
                ArrowPreference(
                    title = stringResource(R.string.app_certificates),
                    onClick = vm::openCertificates,
                )
                ArrowPreference(
                    title = stringResource(R.string.app_app_ids),
                    onClick = vm::openAppIds,
                )
            }
        }
    }
}

@Composable
private fun HeaderCard(onDonate: () -> Unit, onGithub: () -> Unit) {
    Card(Modifier.padding(horizontal = 12.dp).padding(top = 6.dp, bottom = 12.dp)) {
        Row(
            Modifier
                .fillMaxWidth()
                .padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Image(
                painter = painterResource(R.drawable.ic_logo),
                contentDescription = stringResource(R.string.app_logo_alt),
                modifier = Modifier.size(56.dp),
            )
            Spacer(Modifier.width(16.dp))
            Column(Modifier.weight(1f)) {
                Text(stringResource(R.string.native_app_name), style = MiuixTheme.textStyles.title2)
                Text(
                    stringResource(R.string.version) + " " + BuildConfig.VERSION_NAME,
                    style = MiuixTheme.textStyles.body2,
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                )
            }
        }
        Row(
            Modifier.padding(start = 16.dp, end = 16.dp, bottom = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            TextButton(
                text = "❤️ " + stringResource(R.string.app_donate),
                onClick = onDonate,
                modifier = Modifier.weight(1f),
            )
            TextButton(
                text = stringResource(R.string.app_github),
                onClick = onGithub,
                modifier = Modifier.weight(1f),
            )
        }
    }
}

@Composable
private fun AccountContent(vm: AppViewModel) {
    var addAccount by rememberSaveable { mutableStateOf(false) }
    val loggedIn = vm.loggedInAs
    val stored = vm.storedIds
    var confirmDelete by remember { mutableStateOf<String?>(null) }
    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(stringResource(R.string.apple_id_title), style = MiuixTheme.textStyles.title3)
        if (loggedIn != null) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(
                        stringResource(R.string.apple_id_logged_in_as),
                        style = MiuixTheme.textStyles.body2,
                        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    )
                    Text(loggedIn, style = MiuixTheme.textStyles.body1)
                }
                Spacer(Modifier.width(12.dp))
                TextButton(
                    text = stringResource(R.string.apple_id_sign_out),
                    onClick = vm::signOut,
                    colors = ButtonDefaults.textButtonColors(textColor = MiuixTheme.colorScheme.error),
                )
            }
        }
        if (stored.isNotEmpty()) {
            Text(
                stringResource(R.string.apple_id_saved_logins),
                style = MiuixTheme.textStyles.body2,
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            )
            stored.forEach { id ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(id, Modifier.weight(1f), style = MiuixTheme.textStyles.body1)
                    if (loggedIn == null) {
                        TextButton(
                            text = stringResource(R.string.apple_id_sign_in),
                            onClick = { vm.loginStored(id) },
                            colors = ButtonDefaults.textButtonColorsPrimary(),
                            minWidth = 0.dp,
                        )
                        Spacer(Modifier.width(8.dp))
                    }
                    TextButton(
                        text = stringResource(R.string.common_delete),
                        onClick = { confirmDelete = id },
                        colors = ButtonDefaults.textButtonColors(textColor = MiuixTheme.colorScheme.error),
                        minWidth = 0.dp,
                    )
                }
            }
            if (!addAccount && loggedIn == null) {
                TextButton(
                    text = stringResource(R.string.apple_id_add_account),
                    onClick = { addAccount = true },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
        if (loggedIn == null && (stored.isEmpty() || addAccount)) {
            LoginForm(
                vm = vm,
                showTitle = stored.isNotEmpty(),
                onCancel = if (stored.isNotEmpty()) ({ addAccount = false }) else null,
            )
        }
    }
    ConfirmDialog(
        show = confirmDelete != null,
        title = stringResource(R.string.common_delete),
        message = confirmDelete.orEmpty(),
        onConfirm = { confirmDelete?.let(vm::deleteAccount) },
        onDismiss = { confirmDelete = null },
        destructive = true,
    )
}

@Composable
private fun LoginForm(vm: AppViewModel, showTitle: Boolean, onCancel: (() -> Unit)?) {
    var email by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    var save by rememberSaveable { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (showTitle) Text(stringResource(R.string.apple_id_new_login), style = MiuixTheme.textStyles.body1)
        TextField(
            value = email,
            onValueChange = { email = it },
            label = stringResource(R.string.apple_id_email_placeholder),
            useLabelAsPlaceholder = true,
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email),
            modifier = Modifier.fillMaxWidth(),
        )
        TextField(
            value = password,
            onValueChange = { password = it },
            label = stringResource(R.string.apple_id_password_placeholder),
            useLabelAsPlaceholder = true,
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
            modifier = Modifier.fillMaxWidth(),
        )
        if (vm.keyringAvailable) {
            CheckboxPreference(
                title = stringResource(R.string.apple_id_save_credentials),
                checked = save,
                onCheckedChange = { save = it },
                insideMargin = androidx.compose.foundation.layout.PaddingValues(0.dp, 8.dp),
            )
        } else {
            Text(
                stringResource(R.string.apple_id_no_keyring_available),
                style = MiuixTheme.textStyles.body2,
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            )
        }
        TextButton(
            text = stringResource(R.string.apple_id_login),
            onClick = { vm.login(email, password, save && vm.keyringAvailable) },
            modifier = Modifier.fillMaxWidth(),
            colors = ButtonDefaults.textButtonColorsPrimary(),
        )
        if (onCancel != null) {
            TextButton(
                text = stringResource(R.string.common_cancel),
                onClick = onCancel,
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

@Composable
private fun DeviceContent(vm: AppViewModel) {
    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(stringResource(R.string.device_title), style = MiuixTheme.textStyles.title3)
        if (vm.devices.isEmpty()) {
            Text(
                stringResource(R.string.device_no_devices_found_period),
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            )
        }
        vm.devices.forEach { device -> DeviceRow(device, device.id == vm.selectedDevice?.id, !vm.selecting) { vm.selectDevice(device) } }
        TextButton(
            text = stringResource(R.string.common_refresh),
            onClick = vm::loadDevices,
            enabled = !vm.selecting && !vm.devicesLoading,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun DeviceRow(device: DeviceInfo, active: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val container = if (active) MiuixTheme.colorScheme.primaryContainer else MiuixTheme.colorScheme.secondaryVariant
    Card(
        colors = CardDefaults.defaultColors(color = container),
        showIndication = true,
        onClick = if (enabled) onClick else null,
    ) {
        BasicComponent(
            title = device.name,
            summary = stringResource(R.string.native_device_connection, device.connectionType, device.version),
            endActions = {
                if (active) {
                    Text(
                        stringResource(R.string.device_selected),
                        color = MiuixTheme.colorScheme.onPrimaryContainer,
                        style = MiuixTheme.textStyles.body2,
                    )
                }
            },
        )
    }
}
