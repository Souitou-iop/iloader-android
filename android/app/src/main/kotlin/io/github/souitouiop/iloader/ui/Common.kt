package io.github.souitouiop.iloader.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.foundation.layout.Box
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import top.yukonga.miuix.kmp.blur.BlendColorEntry
import top.yukonga.miuix.kmp.blur.BlurDefaults
import top.yukonga.miuix.kmp.blur.LayerBackdrop
import top.yukonga.miuix.kmp.blur.ProgressiveBlur
import top.yukonga.miuix.kmp.blur.isRuntimeShaderSupported
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.blur.progressiveTextureBlur
import top.yukonga.miuix.kmp.blur.rememberLayerBackdrop
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.unit.dp
import io.github.souitouiop.iloader.R
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.overlay.OverlayDialog
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.overScrollVertical

fun Context.openUrl(url: String): Boolean = runCatching {
    startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}.isSuccess

fun Context.copyToClipboard(text: String) {
    val manager = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    manager.setPrimaryClip(ClipData.newPlainText("iloader", text))
}

/** A page with a collapsing miuix top bar and a lazy list. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun PageScaffold(
    title: String,
    onBack: (() -> Unit)?,
    modifier: Modifier = Modifier,
    subtitle: String = "",
    actions: @Composable RowScope.() -> Unit = {},
    listState: LazyListState = rememberLazyListState(),
    content: LazyListScope.() -> Unit,
) {
    val scrollBehavior = MiuixScrollBehavior()
    val imeVisible = WindowInsets.isImeVisible
    val backdrop = rememberBlurBackdrop()
    Scaffold(
        modifier = modifier,
        topBar = {
            BlurredBar(backdrop) {
            TopAppBar(
                title = title,
                subtitle = subtitle,
                color = if (backdrop != null) Color.Transparent else MiuixTheme.colorScheme.surface,
                scrollBehavior = scrollBehavior,
                navigationIcon = {
                    if (onBack != null) {
                        IconButton(onClick = onBack) {
                            Icon(
                                imageVector = MiuixIcons.Back,
                                contentDescription = stringResource(R.string.native_back),
                                tint = MiuixTheme.colorScheme.onBackground,
                            )
                        }
                    }
                },
                actions = actions,
            )
            }
        },
    ) { inner ->
        LazyColumn(
            state = listState,
            modifier = Modifier
                .then(if (backdrop != null) Modifier.layerBackdrop(backdrop) else Modifier)
                .overScrollVertical()
                .nestedScroll(scrollBehavior.nestedScrollConnection)
                .fillMaxHeight(),
            contentPadding = PaddingValues(
                top = inner.calculateTopPadding(),
                bottom = inner.calculateBottomPadding() + 16.dp + if (imeVisible) 160.dp else 0.dp,
            ),
            content = content,
        )
    }
}

/** Cancel + confirm dialog, the equivalent of `DialogContext.confirm`. */
@Composable
fun ConfirmDialog(
    show: Boolean,
    title: String,
    message: String,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
    destructive: Boolean = false,
) {
    OverlayDialog(show = show, title = title, summary = message, onDismissRequest = onDismiss) {
        Row(horizontalArrangement = Arrangement.SpaceBetween) {
            TextButton(
                text = stringResource(R.string.dialog_cancel),
                onClick = onDismiss,
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(20.dp))
            TextButton(
                text = stringResource(R.string.dialog_confirm),
                onClick = {
                    onConfirm()
                    onDismiss()
                },
                modifier = Modifier.weight(1f),
                colors = ButtonDefaults.textButtonColorsPrimary(),
            )
        }
    }
}

/** Text where `<tag>label</tag>` markers become clickable links. */
@Composable
fun taggedLinks(text: String, urls: Map<String, String>, onOpen: (String) -> Unit): AnnotatedString {
    val color = MiuixTheme.colorScheme.primary
    val style = TextLinkStyles(SpanStyle(color = color, textDecoration = TextDecoration.Underline))
    return buildAnnotatedString {
        for (part in io.github.souitouiop.iloader.core.RichText.split(text)) {
            val url = part.tag?.let(urls::get)
            if (url != null) {
                withLink(LinkAnnotation.Clickable("link", style) { onOpen(url) }) { append(part.text) }
            } else {
                append(part.text)
            }
        }
    }
}

/** A suggestion line with `((link:...))` tokens turned into clickable links. */
@Composable
fun suggestionLinks(text: String, onOpen: (String) -> Unit): AnnotatedString {
    val color = MiuixTheme.colorScheme.primary
    val style = TextLinkStyles(SpanStyle(color = color, textDecoration = TextDecoration.Underline))
    return buildAnnotatedString {
        for (segment in io.github.souitouiop.iloader.core.ErrorSuggestions.segments(text)) {
            when (segment) {
                is io.github.souitouiop.iloader.core.ErrorSuggestions.Segment.Text -> append(segment.text)
                is io.github.souitouiop.iloader.core.ErrorSuggestions.Segment.Link ->
                    withLink(LinkAnnotation.Clickable("link", style) { onOpen(segment.url) }) { append(segment.text) }
            }
        }
    }
}

val BoldWeight = FontWeight.SemiBold

/** Backdrop the top bar blurs; null when the device cannot run runtime shaders. */
@Composable
fun rememberBlurBackdrop(): LayerBackdrop? {
    if (!isRuntimeShaderSupported()) return null
    val surface = MiuixTheme.colorScheme.surface
    return rememberLayerBackdrop {
        drawRect(surface)
        drawContent()
    }
}

/** Progressive blur behind the top bar, as in the miuix example app. */
@Composable
fun BlurredBar(backdrop: LayerBackdrop?, content: @Composable () -> Unit) {
    Box {
        if (backdrop != null) {
            Box(
                modifier = Modifier
                    .matchParentSize()
                    .progressiveTextureBlur(
                        backdrop = backdrop,
                        shape = RectangleShape,
                        gradient = ProgressiveBlur.Top.copy(curve = 1.0f),
                        blurRadius = 24f,
                        colors = BlurDefaults.blurColors(
                            blendColors = listOf(BlendColorEntry(color = MiuixTheme.colorScheme.surface.copy(0.75f))),
                        ),
                    ),
            )
        }
        content()
    }
}
