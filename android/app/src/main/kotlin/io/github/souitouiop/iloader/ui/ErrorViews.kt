package io.github.souitouiop.iloader.ui

import androidx.annotation.ArrayRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringArrayResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.github.souitouiop.iloader.R
import io.github.souitouiop.iloader.core.ErrorSuggestions
import io.github.souitouiop.iloader.core.IloaderException
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.overlay.OverlayBottomSheet
import top.yukonga.miuix.kmp.theme.MiuixTheme

private const val DISCORD_URL = "https://discord.gg/EA6yVgydBz"
private const val ISSUES_URL = "https://github.com/Souitou-iop/iloader-android/issues"

@ArrayRes
private fun suggestionArray(block: String): Int? = when (block) {
    "underage" -> R.array.error_suggestions_underage
    "account_locked" -> R.array.error_suggestions_account_locked
    "auth" -> R.array.error_suggestions_auth
    "download" -> R.array.error_suggestions_download
    "house_arrest" -> R.array.error_suggestions_house_arrest
    "pairing" -> R.array.error_suggestions_pairing
    "usbmuxd" -> R.array.error_suggestions_usbmuxd
    "trust" -> R.array.error_suggestions_trust
    "device_coms" -> R.array.error_suggestions_device_coms
    "not_logged_in" -> R.array.error_suggestions_not_logged_in
    "no_device_selected" -> R.array.error_suggestions_no_device_selected
    "anisette" -> R.array.error_suggestions_anisette
    "keyring" -> R.array.error_suggestions_keyring
    "misc" -> R.array.error_suggestions_misc
    "admin" -> R.array.error_suggestions_admin
    "filesystem" -> R.array.error_suggestions_filesystem
    "not_enough_app_ids" -> R.array.error_suggestions_not_enough_app_ids
    "max_apps" -> R.array.error_suggestions_max_apps
    else -> null
}

/** Suggestions for all given error types, de-duplicated, with platform filtering applied. */
@Composable
fun rememberSuggestions(types: List<String>, anisetteServer: String): List<String> {
    val anisetteUrl = ErrorSuggestions.normalizeAnisetteServer(anisetteServer)
    val blocks = types.flatMap(ErrorSuggestions::blocksFor).distinct()
    val lines = blocks.flatMap { block ->
        val array = suggestionArray(block)
        if (array == null) emptyList() else stringArrayResource(array).toList()
    }
    return remember(lines, anisetteUrl) {
        ErrorSuggestions.dedupe(
            lines.mapNotNull { ErrorSuggestions.filterSuggestion(it) }
                .map { it.replace("%1\$s", anisetteUrl) },
        )
    }
}

/** "Suggestions" heading, the bullet list and the support message. */
@Composable
fun SuggestionsBlock(
    types: List<String>,
    anisetteServer: String,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val suggestions = rememberSuggestions(types, anisetteServer)
    val underage = "underage" in types
    Column(modifier) {
        if (suggestions.isNotEmpty()) {
            Text(
                stringResource(R.string.error_suggestions_heading),
                style = MiuixTheme.textStyles.title4,
                fontWeight = BoldWeight,
                modifier = Modifier.padding(bottom = 6.dp),
            )
            suggestions.forEach { line ->
                Row(Modifier.padding(bottom = 4.dp)) {
                    Text("•", Modifier.width(16.dp), color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                    Text(
                        suggestionLinks(line) { context.openUrl(it) },
                        style = MiuixTheme.textStyles.body2,
                    )
                }
            }
            Spacer(Modifier.height(8.dp))
        }
        if (!underage) {
            Text(
                taggedLinks(
                    stringResource(R.string.error_support_message),
                    mapOf("discord" to DISCORD_URL, "github" to ISSUES_URL),
                ) { context.openUrl(it) },
                style = MiuixTheme.textStyles.body2,
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            )
        }
    }
}

/** The raw error message in a monospace box, with the short summary and a "more details" toggle. */
@Composable
fun ErrorMessageBox(error: IloaderException, modifier: Modifier = Modifier) {
    val short = ErrorSuggestions.shortError(error.message)
    val full = ErrorSuggestions.cleanMessage(error.message)
    var expanded by rememberSaveable { mutableStateOf(false) }
    Column(modifier) {
        MonoBox(short ?: full)
        if (short != null) {
            TextButton(
                text = stringResource(R.string.common_more_details) + if (expanded) " ▲" else " ▼",
                onClick = { expanded = !expanded },
                modifier = Modifier.padding(vertical = 8.dp),
                colors = ButtonDefaults.textButtonColors(textColor = MiuixTheme.colorScheme.primary),
                minHeight = 32.dp,
            )
            if (expanded) MonoBox(full)
        }
    }
}

@Composable
private fun MonoBox(text: String) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = top.yukonga.miuix.kmp.basic.CardDefaults.defaultColors(
            color = MiuixTheme.colorScheme.surfaceContainerHigh,
        ),
    ) {
        Text(
            text = text,
            fontFamily = FontFamily.Monospace,
            style = MiuixTheme.textStyles.footnote1,
            modifier = Modifier
                .padding(12.dp)
                .fillMaxWidth(),
        )
    }
}

/** The error dialog: message, per type suggestions, copy. */
@Composable
fun ErrorSheet(state: ErrorState?, anisetteServer: String, onDismiss: () -> Unit, onCopied: () -> Unit) {
    val context = LocalContext.current
    // Keep the last error while the sheet animates out.
    var shown by remember { mutableStateOf<ErrorState?>(null) }
    if (state != null) shown = state
    val current = shown
    OverlayBottomSheet(
        show = state != null,
        title = current?.let { stringResource(R.string.error_title, it.title.resolve()) },
        onDismissRequest = onDismiss,
        onDismissFinished = { shown = null },
    ) {
        if (current != null) {
            Column(
                Modifier
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState()),
            ) {
                ErrorMessageBox(current.error)
                Spacer(Modifier.height(12.dp))
                SuggestionsBlock(listOf(current.error.type), anisetteServer)
                Spacer(Modifier.height(12.dp))
                Row(horizontalArrangement = Arrangement.SpaceBetween) {
                    TextButton(
                        text = stringResource(R.string.common_copy_to_clipboard),
                        onClick = {
                            context.copyToClipboard("```\n" + ErrorSuggestions.cleanMessage(current.error.message) + "\n```")
                            onCopied()
                        },
                        modifier = Modifier.weight(1f),
                    )
                    Spacer(Modifier.width(16.dp))
                    TextButton(
                        text = stringResource(R.string.common_dismiss),
                        onClick = onDismiss,
                        modifier = Modifier.weight(1f),
                        colors = ButtonDefaults.textButtonColorsPrimary(),
                    )
                }
                Spacer(Modifier.height(28.dp))
            }
        }
    }
}
