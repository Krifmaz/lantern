package dev.krifmaz.lantern

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The server address rules: what is tried for what was typed, and what counts as a server. */
class ServerAddressTest {
    /** Without a scheme, https is tried before http; spaces and trailing slashes go. */
    @Test
    fun candidatesWithoutScheme() {
        assertEquals(listOf("https://files.example.com", "http://files.example.com"), ServerAddress.candidates("  files.example.com/ "))
        assertEquals(listOf("https://192.168.1.10:9696", "http://192.168.1.10:9696"), ServerAddress.candidates("192.168.1.10:9696"))
    }

    /** A typed scheme is kept as it is, whatever its case. */
    @Test
    fun candidatesWithScheme() {
        assertEquals(listOf("http://10.0.2.2:9696"), ServerAddress.candidates("http://10.0.2.2:9696//"))
        assertEquals(listOf("HTTPS://Files.example.com"), ServerAddress.candidates("HTTPS://Files.example.com"))
    }

    /** An empty field has nothing to try. */
    @Test
    fun candidatesEmpty() {
        assertTrue(ServerAddress.candidates("   ").isEmpty())
        assertTrue(ServerAddress.candidates("/").isEmpty())
    }

    /** OkHttp's form: lower-case host, no default port, no trailing slash; a server under a path keeps it. */
    @Test
    fun normalise() {
        assertEquals("https://files.example.com", ServerAddress.normalise("HTTPS://Files.Example.com:443"))
        assertEquals("http://10.0.2.2:9696", ServerAddress.normalise("http://10.0.2.2:9696"))
        assertEquals("https://example.com/files", ServerAddress.normalise("https://example.com/files/"))
        assertNull(ServerAddress.normalise("https://exa mple.com"))
        assertNull(ServerAddress.normalise("ftp://example.com"))
    }

    /** A health answer is a server's when it says ok or names its version. */
    @Test
    fun isServer() {
        assertTrue(ServerAddress.isServer(ok = true, version = ""))
        assertTrue(ServerAddress.isServer(ok = false, version = "0.1.0"))
        assertFalse(ServerAddress.isServer(ok = false, version = ""))
    }

    /** "an" before a vowel, "a" otherwise. */
    @Test
    fun notServerWords() {
        assertEquals("That isn't an Orbit server", ServerAddress.notServer("Orbit"))
        assertEquals("That isn't a Filebrary server", ServerAddress.notServer("Filebrary"))
    }
}
