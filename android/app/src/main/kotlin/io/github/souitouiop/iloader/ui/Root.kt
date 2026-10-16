package io.github.souitouiop.iloader.ui

import androidx.activity.compose.BackHandler
import androidx.activity.compose.LocalActivity
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import io.github.souitouiop.iloader.R
import io.github.souitouiop.iloader.data.Prefs
import top.yukonga.miuix.kmp.basic.InfiniteProgressIndicator
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.SnackbarHost
import top.yukonga.miuix.kmp.basic.SnackbarHostState
import top.yukonga.miuix.kmp.basic.Surface
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
fun IloaderRoot(vm: AppViewModel = viewModel()) {
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val anisette by vm.prefs.anisetteServer.collectAsState(initial = Prefs.DEFAULT_ANISETTE)

    LaunchedEffect(Unit) {
        vm.messages.collect { snackbar.showSnackbar(it.resolve(context)) }
    }
    BackHandler(enabled = vm.backStack.size > 1) { vm.back() }

    Scaffold(
        contentWindowInsets = WindowInsets(0),
        snackbarHost = { SnackbarHost(snackbar, Modifier.navigationBarsPadding()) },
    ) {
        Box(Modifier.fillMaxSize().imePadding()) {
            AnimatedContent(
                targetState = vm.route,
                transitionSpec = {
                    // Going deeper keeps the old route on the stack; going back has already popped it.
                    val forward = initialState in vm.backStack
                    if (forward) {
                        (slideInHorizontally(tween(260)) { it / 3 } + fadeIn(tween(260))) togetherWith
                            (slideOutHorizontally(tween(260)) { -it / 6 } + fadeOut(tween(200)))
                    } else {
                        (slideInHorizontally(tween(260)) { -it / 6 } + fadeIn(tween(260))) togetherWith
                            (slideOutHorizontally(tween(260)) { it / 3 } + fadeOut(tween(200)))
                    }.using(SizeTransform(clip = false))
                },
                label = "route",
            ) { route ->
                when (route) {
                    Route.Home -> HomeScreen(vm)
                    Route.Operation -> OperationScreen(vm)
                    Route.Settings -> SettingsScreen(vm)
                    Route.Logs -> LogsScreen(vm)
                    Route.Certificates -> CertificatesScreen(vm)
                    Route.AppIds -> AppIdsScreen(vm)
                    Route.Pairing -> PairingScreen(vm)
                }
            }
            StatusPill(vm.status, Modifier.align(Alignment.BottomCenter))
            TwoFactorDialog(show = vm.twoFactorOpen, onSubmit = vm::submitTwoFactor)
            MaxCertsDialog(state = vm.maxCerts, onRespond = vm::respondMaxCerts)
            PairingDialog(device = vm.pairingDevice, state = vm.pairState, onCancel = vm::cancelPairing)
            ErrorSheet(
                state = vm.error,
                anisetteServer = anisette,
                onDismiss = vm::dismissError,
                onCopied = { vm.toast(UiText(R.string.common_copied_success)) },
            )
        }
    }
}

/** Small "loading" indicator, the counterpart of the pending toast on desktop. */
@Composable
private fun StatusPill(status: UiText?, modifier: Modifier = Modifier) {
    val text = status?.resolve()
    if (text == null) return
    Surface(
        modifier = modifier
            .navigationBarsPadding()
            .padding(bottom = 24.dp),
        shape = RoundedCornerShape(24.dp),
        color = MiuixTheme.colorScheme.surfaceContainerHigh,
        shadowElevation = 6.dp,
    ) {
        Row(
            Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            InfiniteProgressIndicator(color = MiuixTheme.colorScheme.primary, size = 18.dp)
            Text(text, Modifier.padding(start = 12.dp), style = MiuixTheme.textStyles.body2)
        }
    }
}
