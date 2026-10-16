package io.github.souitouiop.iloader.platform

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import io.github.souitouiop.iloader.bridge.BridgeSecretStore
import io.github.souitouiop.iloader.bridge.SecretStoreException
import org.json.JSONObject
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Secrets for the Rust core: every value is encrypted with an AES-GCM key that
 * lives in the Android Keystore and the ciphertexts are kept in one private
 * file. The entry name is bound to its ciphertext as associated data.
 */
class KeystoreSecretStore(context: Context) : BridgeSecretStore {
    private val file = File(context.filesDir, FILE_NAME)
    private val lock = Any()

    override fun get(key: String): String? = guarded {
        val entry = load().optString(key, "")
        if (entry.isEmpty()) null else decrypt(key, Base64.decode(entry, Base64.NO_WRAP))
    }

    override fun set(key: String, value: String) = guarded {
        val entries = load()
        entries.put(key, Base64.encodeToString(encrypt(key, value), Base64.NO_WRAP))
        save(entries)
    }

    override fun delete(key: String): Boolean = guarded {
        val entries = load()
        val existed = entries.remove(key) != null
        if (existed) save(entries)
        existed
    }

    private inline fun <T> guarded(block: () -> T): T = synchronized(lock) {
        try {
            block()
        } catch (e: Exception) {
            throw SecretStoreException.Failed(e.message ?: e.javaClass.simpleName)
        }
    }

    private fun load(): JSONObject =
        if (file.exists()) JSONObject(file.readText()) else JSONObject()

    private fun save(entries: JSONObject) {
        val tmp = File(file.parentFile, "$FILE_NAME.tmp")
        tmp.writeText(entries.toString())
        if (!tmp.renameTo(file)) throw java.io.IOException("could not replace ${file.name}")
    }

    private fun secretKey(): SecretKey {
        val store = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    /** Output: 12 byte IV followed by the ciphertext and tag. */
    private fun encrypt(name: String, plain: String): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, secretKey())
        cipher.updateAAD(name.toByteArray())
        return cipher.iv + cipher.doFinal(plain.toByteArray())
    }

    private fun decrypt(name: String, blob: ByteArray): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(TAG_BITS, blob, 0, IV_BYTES))
        cipher.updateAAD(name.toByteArray())
        return String(cipher.doFinal(blob, IV_BYTES, blob.size - IV_BYTES))
    }

    private companion object {
        const val PROVIDER = "AndroidKeyStore"
        const val KEY_ALIAS = "iloader_secrets_v1"
        const val FILE_NAME = "secrets.json"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
        const val TAG_BITS = 128
    }
}
