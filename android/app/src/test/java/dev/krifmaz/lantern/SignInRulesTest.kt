package dev.krifmaz.lantern

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Test

/** The phone's sign-in: what is sent, how the answer is read, and the phone's name. */
class SignInRulesTest {
    /** The body asks for an app token for this phone; the username is trimmed, the code loses its spaces. */
    @Test
    fun body() {
        val b = SignInRules.body(" ana ", "secret ", "123 456", "d-1", "Google Pixel 8")
        assertEquals(
            Json.parseToJsonElement("""{"username":"ana","password":"secret ","code":"123456","app":{"device_id":"d-1","device_name":"Google Pixel 8"}}"""),
            b,
        )
        assertEquals(null, SignInRules.body("ana", "pw", " ", "d-1", "X")["code"])
        assertEquals(null, SignInRules.body("ana", "pw", null, "d-1", "X")["code"])
    }

    /** A token signs in; an answer without one doesn't. */
    @Test
    fun success() {
        assertEquals(LoginResult.Success("t-1"), SignInRules.result(200, """{"user":{"id":1,"username":"ana"},"token":"t-1"}"""))
        assertEquals(LoginResult.Failed("The server didn't start a session"), SignInRules.result(200, """{"user":{"id":1}}"""))
    }

    /** The server's words are shown as they are; without them, the status. */
    @Test
    fun failures() {
        assertEquals(LoginResult.Failed("Invalid username or password"), SignInRules.result(401, """{"detail":"Invalid username or password"}"""))
        assertEquals(LoginResult.Failed("Sign-in failed (HTTP 502)"), SignInRules.result(502, "<html>Bad Gateway</html>"))
        assertEquals(LoginResult.NeedsCode("Enter your code"), SignInRules.result(401, """{"detail":"Enter your code","two_factor":true}"""))
    }

    /** Maker and model, the model alone when it already names the maker. */
    @Test
    fun deviceName() {
        assertEquals("Google Pixel 8", SignInRules.deviceName("google", "Pixel 8"))
        assertEquals("SM-S911B", SignInRules.deviceName("", "SM-S911B"))
        assertEquals("Samsung Galaxy", SignInRules.deviceName("Samsung", "Samsung Galaxy"))
        assertEquals("Android", SignInRules.deviceName("", ""))
    }

    /** The page's title. */
    @Test
    fun title() {
        assertEquals("Sign in to Filebrary", SignInRules.title("Filebrary"))
    }
}
