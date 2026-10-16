package io.github.souitouiop.iloader.ui

import android.content.Context
import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext

/** A string resource with arguments, so state holders stay free of a Context. */
class UiText(@StringRes val id: Int, vararg val args: Any) {
    fun resolve(context: Context): String = context.getString(id, *args)

    @Composable
    fun resolve(): String = resolve(LocalContext.current)
}
