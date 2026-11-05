package dev.krifmaz.lantern

import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/**
 * The server's address as a person types it, turned into the base URL the app calls (the kit's sign-in
 * rules). Plain Kotlin, so the unit tests run it on the JVM; [ServerApi.probe] does the asking.
 */
object ServerAddress {
    /** Shown when the field is empty. */
    const val EMPTY = "Enter your server's address"

    /** Shown when the text can't be a web address. */
    const val INVALID = "That isn't a valid address"

    /** Shown when nothing answered. */
    const val UNREACHABLE = "Can't reach that server"

    /**
     * The addresses to try, in order, for what was typed: as it is when it starts with http:// or https://,
     * otherwise https:// first and then http:// (a server on the LAN is often plain http). Spaces around it and
     * trailing slashes are dropped; nothing to try for an empty field.
     */
    fun candidates(input: String): List<String> {
        val raw = input.trim().trimEnd('/')
        if (raw.isEmpty()) return emptyList()
        val lower = raw.lowercase()
        return if (lower.startsWith("http://") || lower.startsWith("https://")) listOf(raw) else listOf("https://$raw", "http://$raw")
    }

    /**
     * OkHttp's form of [candidate] without a trailing slash (lower-case host, no default port), so the requests the
     * app makes later match it; null when it isn't a web address.
     */
    fun normalise(candidate: String): String? = candidate.toHttpUrlOrNull()?.toString()?.trimEnd('/')

    /** Whether a `/api/health` answer comes from a server: it says `ok`, or names its version. */
    fun isServer(ok: Boolean, version: String): Boolean = ok || version.isNotEmpty()

    /** Shown when something answered but isn't this app's server ("That isn't an Orbit server"). */
    fun notServer(app: String): String = "That isn't ${article(app)} $app server"

    /** "an" before a vowel ("an Orbit server"), "a" otherwise ("a Filebrary server"). */
    private fun article(word: String): String = if (word.firstOrNull()?.lowercaseChar() in setOf('a', 'e', 'i', 'o', 'u')) "an" else "a"
}
