package io.github.souitouiop.iloader.data

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

private val Context.dataStore: DataStore<Preferences> by preferencesDataStore(name = "settings")

/** User preferences owned by the Kotlin side (see the contract). */
class Prefs(context: Context) {
    private val store = context.applicationContext.dataStore

    val anisetteServer: Flow<String> = store.data.map { it[ANISETTE] ?: DEFAULT_ANISETTE }
    val overrideKeyring: Flow<Boolean> = store.data.map { it[OVERRIDE_KEYRING] ?: false }

    /** `system`, or a language tag of [LANGUAGES]. */
    val lang: Flow<String> = store.data.map { it[LANG] ?: LANG_SYSTEM }

    suspend fun setAnisetteServer(value: String) = store.edit { it[ANISETTE] = value }
    suspend fun setOverrideKeyring(value: Boolean) = store.edit { it[OVERRIDE_KEYRING] = value }
    suspend fun setLang(value: String) = store.edit { it[LANG] = value }

    companion object {
        const val DEFAULT_ANISETTE = "ani.jkcoxson.com"
        const val LANG_SYSTEM = "system"

        /** Anisette presets: server to display name. */
        val ANISETTE_PRESETS = listOf(
            "ani.jkcoxson.com" to "Jkcoxson",
            "ani2.stikstore.app" to "StikStore (secondary)",
        )

        /** Selectable languages: tag to own-language name. */
        val LANGUAGES = listOf("en" to "English", "zh-CN" to "简体中文")

        private val ANISETTE = stringPreferencesKey("anisetteServer")
        private val OVERRIDE_KEYRING = booleanPreferencesKey("overrideKeyring")
        private val LANG = stringPreferencesKey("lang")
    }
}
