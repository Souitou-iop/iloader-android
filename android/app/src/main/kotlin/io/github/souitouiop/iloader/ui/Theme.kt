package io.github.souitouiop.iloader.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import top.yukonga.miuix.kmp.theme.ColorSchemeMode
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.ThemeController

/** miuix default color scheme (no Monet), following the system dark mode. */
@Composable
fun IloaderTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val controller = remember(dark) { ThemeController(ColorSchemeMode.System, isDark = dark) }
    MiuixTheme(controller = controller, content = content)
}
