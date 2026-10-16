package io.github.souitouiop.iloader.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.github.souitouiop.iloader.R
import io.github.souitouiop.iloader.core.ErrorSuggestions
import io.github.souitouiop.iloader.data.Prefs
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.CircularProgressIndicator
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.InfiniteProgressIndicator
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Close
import top.yukonga.miuix.kmp.icon.extended.Ok
import top.yukonga.miuix.kmp.icon.extended.Remove
import top.yukonga.miuix.kmp.theme.MiuixTheme

private enum class StepStatus { Waiting, Running, Done, Failed, Skipped }

@Composable
private fun StepIcon(status: StepStatus) {
    Box(Modifier.size(28.dp), contentAlignment = Alignment.Center) {
        when (status) {
            StepStatus.Waiting -> CircularProgressIndicator(progress = 0f, size = 18.dp, strokeWidth = 2.dp)
            StepStatus.Running -> InfiniteProgressIndicator(color = MiuixTheme.colorScheme.primary, size = 22.dp)
            StepStatus.Done -> Icon(MiuixIcons.Ok, null, tint = MiuixTheme.colorScheme.primary)
            StepStatus.Failed -> Icon(MiuixIcons.Close, null, tint = MiuixTheme.colorScheme.error)
            StepStatus.Skipped -> Icon(MiuixIcons.Remove, null, tint = MiuixTheme.colorScheme.onSurfaceVariantSummary)
        }
    }
}

@Composable
fun OperationScreen(vm: AppViewModel) {
    val state = vm.operation ?: return
    val context = LocalContext.current
    val anisette by vm.prefs.anisetteServer.collectAsState(initial = Prefs.DEFAULT_ANISETTE)
    val kind = state.kind
    val done = state.done
    val failed = state.hasFailed
    val title = if (done && !failed && kind.successTitleRes != null) kind.successTitleRes else kind.titleRes
    val subtitle = when {
        done && failed -> R.string.operation_failed
        done -> R.string.operation_completed
        else -> R.string.operation_please_wait
    }
    PageScaffold(
        title = stringResource(title),
        subtitle = stringResource(subtitle),
        onBack = if (done) ({ vm.closeOperation() }) else null,
    ) {
        item(key = "steps") {
            Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    kind.steps.forEach { step ->
                        val failure = state.failed.firstOrNull { it.stepId == step.id }
                        val status = when {
                            failure != null -> StepStatus.Failed
                            step.id in state.completed -> StepStatus.Done
                            step.id in state.started -> StepStatus.Running
                            failed -> StepStatus.Skipped
                            else -> StepStatus.Waiting
                        }
                        Row(verticalAlignment = Alignment.Top) {
                            StepIcon(status)
                            Column(Modifier.weight(1f).padding(start = 12.dp, top = 3.dp)) {
                                Text(
                                    stringResource(step.titleRes),
                                    style = MiuixTheme.textStyles.body1,
                                    color = if (status == StepStatus.Waiting || status == StepStatus.Skipped) {
                                        MiuixTheme.colorScheme.onSurfaceVariantSummary
                                    } else {
                                        MiuixTheme.colorScheme.onSurface
                                    },
                                )
                                if (failure != null) {
                                    ErrorMessageBox(failure.error, Modifier.padding(top = 8.dp))
                                }
                            }
                        }
                    }
                }
            }
        }
        if (done && !failed && kind.successMessageRes != null) {
            item(key = "success") {
                Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                    Text(stringResource(kind.successMessageRes), Modifier.padding(16.dp))
                }
            }
        }
        if (done && failed) {
            item(key = "suggestions") {
                Card(Modifier.padding(horizontal = 12.dp).padding(bottom = 12.dp)) {
                    SuggestionsBlock(
                        types = state.failed.map { it.error.type },
                        anisetteServer = anisette,
                        modifier = Modifier.padding(16.dp),
                    )
                }
            }
            item(key = "copy") {
                TextButton(
                    text = stringResource(R.string.operation_copy_error_clipboard),
                    onClick = {
                        val message = state.failed.firstOrNull()?.error?.message?.let(ErrorSuggestions::cleanMessage)
                            ?: context.getString(R.string.common_no_error)
                        context.copyToClipboard("```\n$message\n```")
                        vm.toast(UiText(R.string.common_copied_success))
                    },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp).padding(bottom = 12.dp),
                    colors = ButtonDefaults.textButtonColorsPrimary(),
                )
            }
        }
        if (done) {
            item(key = "dismiss") {
                TextButton(
                    text = stringResource(R.string.common_dismiss),
                    onClick = vm::closeOperation,
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
                )
            }
        }
    }
}
