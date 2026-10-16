package io.github.souitouiop.iloader.core

/** Port of `src/errors.tsx`: maps an `AppError` type to suggestion blocks and parses their markup. */
object ErrorSuggestions {
    /** Error type -> suggestion block names (`error.suggestions.<block>` in the locale files). */
    val blocksByType: Map<String, List<String>> = mapOf(
        "underage" to listOf("underage"),
        "account_locked" to listOf("account_locked"),
        "developer" to emptyList(),
        "auth" to listOf("auth"),
        "download" to listOf("download"),
        "house_arrest" to listOf("house_arrest", "trust", "device_coms"),
        "remote_pairing" to listOf("trust", "pairing"),
        "lockdown_pairing" to listOf("trust", "pairing"),
        "canceled" to emptyList(),
        "operation_update" to emptyList(),
        "device_coms" to listOf("device_coms", "trust"),
        "device_coms_with_message" to listOf("device_coms", "trust"),
        "usbmuxd" to listOf("usbmuxd", "device_coms", "trust"),
        "not_logged_in" to listOf("not_logged_in"),
        "no_device_selected" to listOf("no_device_selected"),
        "anisette" to listOf("anisette"),
        "keyring" to listOf("keyring", "admin"),
        "keyring_with_message" to listOf("keyring", "admin"),
        "storage" to listOf("keyring", "admin", "filesystem"),
        "misc" to listOf("misc"),
        "filesystem" to listOf("filesystem", "admin"),
        "not_enough_app_ids" to listOf("not_enough_app_ids"),
        "max_apps" to listOf("max_apps"),
    )

    const val PLATFORM = "android"

    fun blocksFor(type: String): List<String> = blocksByType[type].orEmpty()

    fun normalizeAnisetteServer(server: String): String =
        if (server.startsWith("http://") || server.startsWith("https://")) server else "https://$server"

    /**
     * Applies the `[platform::x]` and `[ios::x]` prefixes. Returns null when the
     * suggestion is meant for another platform.
     */
    fun filterSuggestion(raw: String, platform: String = PLATFORM): String? {
        if (raw.startsWith("[platform::")) {
            val end = raw.indexOf(']')
            if (end == -1 || raw.substring(11, end) != platform) return null
        }
        return raw
            .replace(Regex("^\\[platform::.*?]"), "")
            // TODO: actually check the iOS version, like the desktop UI
            .replace(Regex("\\[ios::.*?]"), "")
            .trim()
    }

    fun dedupe(suggestions: List<String>): List<String> = suggestions.distinct()

    /** A piece of suggestion text: plain, or a link to open. */
    sealed interface Segment {
        data class Text(val text: String) : Segment
        data class Link(val url: String, val text: String) : Segment
    }

    private val linkToken = Regex("\\(\\(link:[^)]+\\)\\)")

    /** Splits `((link:url:text))` / `((link::url))` tokens out of [text]. */
    fun segments(text: String): List<Segment> {
        val out = mutableListOf<Segment>()
        var last = 0
        for (match in linkToken.findAll(text)) {
            if (match.range.first > last) out += Segment.Text(text.substring(last, match.range.first))
            val link = parseLinkToken(match.value)
            out += link ?: Segment.Text(match.value)
            last = match.range.last + 1
        }
        if (last < text.length) out += Segment.Text(text.substring(last))
        return out
    }

    fun parseLinkToken(token: String): Segment.Link? {
        Regex("^\\(\\(link::([^)]+)\\)\\)$").find(token)?.let {
            val url = it.groupValues[1].trim()
            return if (url.isEmpty()) null else Segment.Link(url, url)
        }
        val payload = Regex("^\\(\\(link:([^)]+)\\)\\)$").find(token)?.groupValues?.get(1)?.trim()
        if (payload.isNullOrEmpty()) return null
        val lastColon = payload.lastIndexOf(':')
        if (lastColon > 0) {
            val url = payload.substring(0, lastColon).trim()
            val text = payload.substring(lastColon + 1).trim()
            if (text.isNotEmpty() && Regex("^[a-z][a-z0-9+.-]*://", RegexOption.IGNORE_CASE).containsMatchIn(url)) {
                return Segment.Link(url, text)
            }
        }
        return Segment.Link(payload, payload)
    }

    /** The last line containing "●" without the marker, as the desktop UI shows it. */
    fun shortError(message: String): String? =
        message.lines().lastOrNull { it.contains('●') }?.replace(Regex("●\\s*"), "")?.trim()?.ifEmpty { null }

    fun cleanMessage(message: String): String = message.trimStart('\n')
}
