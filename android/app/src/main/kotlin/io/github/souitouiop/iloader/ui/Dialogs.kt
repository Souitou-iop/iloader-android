package io.github.souitouiop.iloader.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import kotlinx.coroutines.delay
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.souitouiop.iloader.R
import io.github.souitouiop.iloader.core.DeviceInfo
import io.github.souitouiop.iloader.core.PairState
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.InfiniteProgressIndicator
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.overlay.OverlayDialog
import top.yukonga.miuix.kmp.preference.CheckboxPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
fun TwoFactorDialog(show: Boolean, onSubmit: (String) -> Boolean) {
    var code by rememberSaveable { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    LaunchedEffect(show) {
        if (show) {
            delay(350)
            runCatching { focus.requestFocus() }
        }
    }
    OverlayDialog(
        show = show,
        title = stringResource(R.string.apple_id_two_factor_title),
        summary = stringResource(R.string.apple_id_two_factor_prompt),
    ) {
        TextField(
            value = code,
            onValueChange = { code = it.filter(Char::isDigit).take(6) },
            label = stringResource(R.string.apple_id_verification_placeholder),
            useLabelAsPlaceholder = true,
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
            modifier = Modifier.fillMaxWidth().focusRequester(focus),
        )
        Spacer(Modifier.height(16.dp))
        TextButton(
            text = stringResource(R.string.apple_id_submit),
            onClick = { if (onSubmit(code)) code = "" },
            modifier = Modifier.fillMaxWidth(),
            colors = ButtonDefaults.textButtonColorsPrimary(),
        )
    }
}

/** "Maximum certificates reached": pick which ones to revoke, everything is selected by default. */
@Composable
fun MaxCertsDialog(state: MaxCertsState?, onRespond: (List<String>?) -> Unit) {
    var shown by remember { mutableStateOf<MaxCertsState?>(null) }
    if (state != null) shown = state
    val certs = shown?.certificates.orEmpty()
    var expanded by rememberSaveable { mutableStateOf(false) }
    var selected by remember(shown) { mutableStateOf(certs.map { it.serialNumber.orEmpty() }.toSet()) }
    OverlayDialog(
        show = state != null,
        title = stringResource(R.string.apple_id_max_certs_title),
        summary = stringResource(R.string.apple_id_max_certs_desc),
        onDismissFinished = { shown = null },
    ) {
        TextButton(
            text = stringResource(
                if (expanded) R.string.apple_id_hide_certificate_list else R.string.apple_id_choose_what_to_revoke,
            ),
            onClick = { expanded = !expanded },
            modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp),
        )
        if (expanded) {
            Card(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(bottom = 12.dp)
                    .heightIn(max = 280.dp),
            ) {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    certs.forEach { cert ->
                        val serial = cert.serialNumber.orEmpty()
                        CheckboxPreference(
                            title = cert.name.orEmpty(),
                            summary = cert.machineName,
                            checked = serial in selected,
                            onCheckedChange = { on -> selected = if (on) selected + serial else selected - serial },
                        )
                    }
                }
            }
        }
        Row(horizontalArrangement = Arrangement.SpaceBetween) {
            TextButton(
                text = stringResource(R.string.common_cancel),
                onClick = { onRespond(null) },
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(20.dp))
            TextButton(
                text = stringResource(R.string.apple_id_continue),
                onClick = { onRespond(selected.toList()) },
                modifier = Modifier.weight(1f),
                colors = ButtonDefaults.textButtonColorsPrimary(),
            )
        }
    }
}

/** Shown while the device is being paired; the iPhone shows the "Trust" prompt. */
@Composable
fun PairingDialog(device: DeviceInfo?, state: PairState, onCancel: () -> Unit) {
    var shown by remember { mutableStateOf<DeviceInfo?>(null) }
    if (device != null) shown = device
    OverlayDialog(
        show = device != null,
        title = stringResource(R.string.device_pairing_in_progress_header, shown?.name ?: ""),
        onDismissRequest = onCancel,
        onDismissFinished = { shown = null },
    ) {
        Box(Modifier.fillMaxWidth().padding(bottom = 12.dp), contentAlignment = Alignment.Center) {
            InfiniteProgressIndicator(color = MiuixTheme.colorScheme.primary)
        }
        Text(
            text = when (state) {
                PairState.Pairing -> stringResource(R.string.native_pair_connecting)
                PairState.WaitingForTrust, PairState.Paired -> stringResource(R.string.device_pairing_in_progress_hint)
                PairState.Denied -> stringResource(R.string.native_pair_denied)
                is PairState.Failed -> stringResource(R.string.native_pair_failed, state.message)
            },
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            modifier = Modifier.padding(bottom = 16.dp),
        )
        TextButton(
            text = stringResource(R.string.device_pairing_cancel),
            onClick = onCancel,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}
