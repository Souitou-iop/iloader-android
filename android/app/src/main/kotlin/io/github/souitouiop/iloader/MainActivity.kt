package io.github.souitouiop.iloader

import android.content.Context
import android.content.res.Configuration
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.DisposableEffect
import io.github.souitouiop.iloader.data.Prefs
import io.github.souitouiop.iloader.ui.IloaderRoot
import io.github.souitouiop.iloader.ui.IloaderTheme
import java.util.Locale
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking

class MainActivity : ComponentActivity() {
    /** Applies the language chosen in the settings (the system default when unset). */
    override fun attachBaseContext(base: Context) {
        val prefs = (base.applicationContext as? IloaderApp)?.prefs
        val lang = prefs?.let { runBlocking { it.lang.first() } } ?: Prefs.LANG_SYSTEM
        if (lang == Prefs.LANG_SYSTEM) {
            super.attachBaseContext(base)
            return
        }
        val config = Configuration(base.resources.configuration)
        config.setLocale(Locale.forLanguageTag(lang))
        super.attachBaseContext(base.createConfigurationContext(config))
    }

    override fun onResume() {
        super.onResume()
        // Picks up an iPhone that was plugged in while the app was away.
        (application as? IloaderApp)?.usbHost?.rescan()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            val dark = isSystemInDarkTheme()
            DisposableEffect(dark) {
                enableEdgeToEdge(
                    statusBarStyle = SystemBarStyle.auto(Color.TRANSPARENT, Color.TRANSPARENT) { dark },
                    navigationBarStyle = SystemBarStyle.auto(Color.TRANSPARENT, Color.TRANSPARENT) { dark },
                )
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    window.isNavigationBarContrastEnforced = false
                }
                onDispose {}
            }
            IloaderTheme { IloaderRoot() }
        }
    }
}
