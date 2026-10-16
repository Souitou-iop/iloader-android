package io.github.souitouiop.iloader.ui

import android.app.Activity
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import io.github.souitouiop.iloader.R
import io.github.souitouiop.iloader.core.LogRecord
import io.github.souitouiop.iloader.data.Prefs
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.SmallTitle
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Copy
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.preference.OverlayDropdownPreference
import top.yukonga.miuix.kmp.preference.SwitchPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
fun SettingsScreen(vm: AppViewModel) {
    val context = LocalContext.current
    val anisette by vm.prefs.anisetteServer.collectAsState(initial = Prefs.DEFAULT_ANISETTE)
    val lang by vm.prefs.lang.collectAsState(initial = Prefs.LANG_SYSTEM)
    var confirmReset by remember { mutableStateOf(false) }
    var confirmDeletePairing by remember { mutableStateOf(false) }

    val presets = Prefs.ANISETTE_PRESETS
    val customIndex = presets.size
    var customMode by rememberSaveable { mutableStateOf(presets.none { it.first == anisette } && anisette != Prefs.DEFAULT_ANISETTE) }
    val anisetteIndex = if (customMode) customIndex else presets.indexOfFirst { it.first == anisette }.coerceAtLeast(0)
    val anisetteItems = presets.map { it.second } + stringResource(R.string.settings_custom_anisette)

    val languageItems = listOf(stringResource(R.string.native_language_system)) + Prefs.LANGUAGES.map { it.second }
    val languageIndex = if (lang == Prefs.LANG_SYSTEM) 0 else Prefs.LANGUAGES.indexOfFirst { it.first == lang } + 1

    PageScaffold(title = stringResource(R.string.app_settings), onBack = { vm.back() }) {
        item(key = "server") {
            SmallTitle(stringResource(R.string.native_settings_section_server))
            Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                OverlayDropdownPreference(
                    title = stringResource(R.string.settings_anisette_server).trimEnd(':', ' '),
                    summary = anisette,
                    items = anisetteItems,
                    selectedIndex = anisetteIndex,
                    onSelectedIndexChange = { index ->
                        if (index == customIndex) {
                            customMode = true
                        } else {
                            customMode = false
                            vm.setAnisetteServer(presets[index].first)
                        }
                    },
                )
                if (customMode) {
                    var text by rememberSaveable { mutableStateOf(anisette) }
                    TextField(
                        value = text,
                        onValueChange = {
                            text = it
                            vm.setAnisetteServer(it)
                        },
                        label = stringResource(R.string.settings_custom_anisette_placeholder),
                        useLabelAsPlaceholder = true,
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 12.dp),
                    )
                }
                OverlayDropdownPreference(
                    title = stringResource(R.string.app_language),
                    items = languageItems,
                    selectedIndex = languageIndex.coerceAtLeast(0),
                    onSelectedIndexChange = { index ->
                        val tag = if (index == 0) Prefs.LANG_SYSTEM else Prefs.LANGUAGES[index - 1].first
                        vm.setLang(tag)
                        // Recreate once the preference is stored so attachBaseContext picks it up.
                        (context as? Activity)?.window?.decorView?.postDelayed({ (context as Activity).recreate() }, 300)
                    },
                )
                Text(
                    taggedLinks(
                        stringResource(R.string.settings_language_hint),
                        mapOf("translation" to "https://github.com/nab138/iloader?tab=readme-ov-file#translating"),
                    ) { context.openUrl(it) },
                    style = MiuixTheme.textStyles.footnote1,
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    modifier = Modifier.padding(horizontal = 16.dp).padding(bottom = 12.dp),
                )
            }
        }
        item(key = "actions") {
            SmallTitle(stringResource(R.string.native_settings_section_actions))
            Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                ArrowPreference(
                    title = stringResource(R.string.settings_reset_anisette_title),
                    onClick = { confirmReset = true },
                )
                ArrowPreference(
                    title = stringResource(R.string.settings_delete_stored_rppairing),
                    onClick = { if (vm.selectedDevice == null) vm.toast(UiText(R.string.app_must_select_device)) else confirmDeletePairing = true },
                )
                ArrowPreference(
                    title = stringResource(R.string.settings_view_logs),
                    onClick = { vm.navigate(Route.Logs) },
                )
            }
        }
        item(key = "security") {
            SmallTitle(stringResource(R.string.native_settings_section_security))
            Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                SwitchPreference(
                    title = stringResource(R.string.settings_dont_use_keyring),
                    summary = stringResource(R.string.settings_dont_use_keyring_message),
                    checked = vm.overrideKeyring,
                    onCheckedChange = vm::changeOverrideKeyring,
                )
            }
        }
    }
    ConfirmDialog(
        show = confirmReset,
        title = stringResource(R.string.settings_reset_anisette_title),
        message = stringResource(R.string.settings_reset_anisette_message),
        onConfirm = vm::resetAnisette,
        onDismiss = { confirmReset = false },
        destructive = true,
    )
    ConfirmDialog(
        show = confirmDeletePairing,
        title = stringResource(R.string.settings_delete_stored_rppairing),
        message = stringResource(R.string.settings_delete_stored_rppairing_message),
        onConfirm = vm::deleteStoredPairing,
        onDismiss = { confirmDeletePairing = false },
        destructive = true,
    )
}

private fun levelName(level: Int) = when (level) {
    LogRecord.TRACE -> "TRACE"
    LogRecord.DEBUG -> "DEBUG"
    LogRecord.INFO -> "INFO"
    LogRecord.WARN -> "WARN"
    LogRecord.ERROR -> "ERROR"
    else -> "UNKNOWN"
}

@Composable
private fun levelColor(level: Int): Color = when (level) {
    LogRecord.TRACE, LogRecord.DEBUG -> MiuixTheme.colorScheme.onSurfaceVariantSummary
    LogRecord.INFO -> MiuixTheme.colorScheme.primary
    LogRecord.WARN -> MiuixTheme.colorScheme.onTertiaryContainer
    else -> MiuixTheme.colorScheme.error
}

@Composable
fun LogsScreen(vm: AppViewModel) {
    val context = LocalContext.current
    val levels = listOf(LogRecord.DEBUG, LogRecord.INFO, LogRecord.WARN, LogRecord.ERROR)
    val levelItems = listOf(
        stringResource(R.string.settings_debug),
        stringResource(R.string.settings_info),
        stringResource(R.string.settings_warn),
        stringResource(R.string.settings_error),
    )
    var filter by rememberSaveable { mutableStateOf(1) }
    val filtered = vm.logs.filter { it.level >= levels[filter] }
    val listState = rememberLazyListState()
    LaunchedEffect(filtered.size) {
        if (filtered.isNotEmpty()) listState.animateScrollToItem(filtered.lastIndex)
    }
    PageScaffold(
        title = stringResource(R.string.settings_logs),
        onBack = { vm.back() },
        actions = {
            IconButton(
                onClick = {
                    val text = filtered.joinToString("\n") { "[${it.timestamp}] [${levelName(it.level)}] ${it.target?.let { t -> "<$t> " }.orEmpty()}${it.message}" }
                    context.copyToClipboard("```\n$text\n```")
                    vm.toast(UiText(R.string.common_copied_success))
                },
            ) {
                Icon(
                    imageVector = MiuixIcons.Copy,
                    contentDescription = stringResource(R.string.common_copy_to_clipboard),
                    tint = MiuixTheme.colorScheme.onBackground,
                )
            }
        },
        listState = listState,
    ) {
        item(key = "level") {
            Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                OverlayDropdownPreference(
                    title = stringResource(R.string.settings_log_level).trimEnd(':', ' '),
                    items = levelItems,
                    selectedIndex = filter,
                    onSelectedIndexChange = { filter = it },
                )
            }
        }
        if (filtered.isEmpty()) {
            item(key = "empty") {
                Text(
                    stringResource(R.string.settings_no_logs_yet),
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    modifier = Modifier.padding(horizontal = 28.dp),
                )
            }
        } else {
            items(filtered) { log ->
                val infoColor = MiuixTheme.colorScheme.onSurfaceVariantSummary
                val levelTint = levelColor(log.level)
                Text(
                    text = buildAnnotatedString {
                        withStyle(SpanStyle(color = infoColor)) { append("[${log.timestamp.take(19)}] ") }
                        withStyle(SpanStyle(color = levelTint)) { append("[${levelName(log.level)}] ") }
                        log.target?.let {
                            withStyle(SpanStyle(color = infoColor)) { append("$it ") }
                        }
                        append(log.message)
                    },
                    fontFamily = FontFamily.Monospace,
                    style = MiuixTheme.textStyles.footnote2,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 2.dp),
                )
            }
        }
    }
}
