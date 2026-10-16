package io.github.souitouiop.iloader.core

import io.github.souitouiop.iloader.core.ErrorSuggestions.Segment
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ErrorSuggestionsTest {
    @Test
    fun everyRustErrorVariantHasAMapping() {
        val variants = listOf(
            "max_apps", "not_enough_app_ids", "device_coms", "underage", "account_locked", "developer", "auth",
            "download", "house_arrest", "remote_pairing", "lockdown_pairing", "canceled", "operation_update",
            "device_coms_with_message", "usbmuxd", "not_logged_in", "no_device_selected", "anisette", "keyring",
            "keyring_with_message", "storage", "misc", "filesystem",
        )
        variants.forEach { assertTrue(it in ErrorSuggestions.blocksByType, "missing $it") }
        assertEquals(variants.size, ErrorSuggestions.blocksByType.size)
    }

    @Test
    fun mappingMatchesDesktop() {
        assertEquals(listOf("house_arrest", "trust", "device_coms"), ErrorSuggestions.blocksFor("house_arrest"))
        assertEquals(listOf("keyring", "admin", "filesystem"), ErrorSuggestions.blocksFor("storage"))
        assertEquals(emptyList(), ErrorSuggestions.blocksFor("developer"))
        assertEquals(emptyList(), ErrorSuggestions.blocksFor("unknown_type"))
    }

    @Test
    fun platformPrefixesAreFiltered() {
        assertNull(ErrorSuggestions.filterSuggestion("[platform::windows]Install iTunes", "android"))
        assertEquals("Do it", ErrorSuggestions.filterSuggestion("[platform::android]Do it", "android"))
        assertEquals("Plain", ErrorSuggestions.filterSuggestion("Plain", "android"))
        assertEquals("No beta", ErrorSuggestions.filterSuggestion("[ios::26.3]No beta", "android"))
    }

    @Test
    fun anisetteServerGetsAScheme() {
        assertEquals("https://ani.example.com", ErrorSuggestions.normalizeAnisetteServer("ani.example.com"))
        assertEquals("http://localhost:6969", ErrorSuggestions.normalizeAnisetteServer("http://localhost:6969"))
    }

    @Test
    fun linkTokens() {
        assertEquals(Segment.Link("https://apple.co/ms", "iTunes"), ErrorSuggestions.parseLinkToken("((link:https://apple.co/ms:iTunes))"))
        assertEquals(Segment.Link("https://iforgot.apple.com/", "https://iforgot.apple.com/"), ErrorSuggestions.parseLinkToken("((link:https://iforgot.apple.com/))"))
        assertEquals(Segment.Link("https://a.b/c", "https://a.b/c"), ErrorSuggestions.parseLinkToken("((link::https://a.b/c))"))
        assertNull(ErrorSuggestions.parseLinkToken("((link:))"))
    }

    @Test
    fun segmentsSplitTextAndLinks() {
        val parts = ErrorSuggestions.segments("Visit ((link:https://iforgot.apple.com/)) to unlock.")
        assertEquals(
            listOf(
                Segment.Text("Visit "),
                Segment.Link("https://iforgot.apple.com/", "https://iforgot.apple.com/"),
                Segment.Text(" to unlock."),
            ),
            parts,
        )
    }

    @Test
    fun shortErrorTakesTheLastBulletLine() {
        val message = "\nLogin failed\n● first\n● Your password was incorrect. (-20101)"
        assertEquals("Your password was incorrect. (-20101)", ErrorSuggestions.shortError(message))
        assertNull(ErrorSuggestions.shortError("plain message"))
        assertEquals("Login failed\n● x", ErrorSuggestions.cleanMessage("\n\nLogin failed\n● x"))
    }

    @Test
    fun richTextSplitsTags() {
        val parts = RichText.split("Send it to <discord>Discord</discord> or a <github>issue</github>.")
        assertEquals(
            listOf(
                RichText.Part("Send it to "),
                RichText.Part("Discord", "discord"),
                RichText.Part(" or a "),
                RichText.Part("issue", "github"),
                RichText.Part("."),
            ),
            parts,
        )
    }
}
