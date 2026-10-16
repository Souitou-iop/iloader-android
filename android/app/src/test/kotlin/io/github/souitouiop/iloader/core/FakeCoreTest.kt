package io.github.souitouiop.iloader.core

import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class FakeCoreTest {
    private fun core() = FakeCore(speed = 0.0)

    /** Collects all events into a list for the rest of the test. */
    private fun TestScope.record(core: FakeCore): List<CoreEvent> {
        val events = mutableListOf<CoreEvent>()
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { core.events.collect { events += it } }
        return events
    }

    private suspend fun FakeCore.loginAndSelect(email: String) {
        loginNew(email, "pw", "x", false)
        selectDevice((listDevices().first() as DeviceResult.Ok).device)
    }

    @Test
    fun loginAsksForTwoFactorThenSucceeds() = runTest {
        val core = core()
        val events = record(core)
        val login = launch { core.loginNew("me@example.com", "secret", "ani.example.com", true) }
        runCurrent()
        assertTrue(events.any { it is CoreEvent.TwoFactorRequired })
        core.submitTwoFactor("123456")
        login.join()
        assertEquals("me@example.com", core.loggedInAs())
        assertTrue("me@example.com" in core.storedAccounts())
    }

    @Test
    fun wrongTwoFactorCodeIsRejected() = runTest {
        val core = core()
        val login = launch {
            val e = assertFailsWith<IloaderException> { core.loginNew("me@example.com", "secret", "x", false) }
            assertEquals("auth", e.type)
        }
        runCurrent()
        core.submitTwoFactor("000000")
        login.join()
        assertNull(core.loggedInAs())
    }

    @Test
    fun maxCertsCanBeAnsweredAndRevokes() = runTest {
        val core = core()
        val events = record(core)
        val login = launch { core.loginNew("certs.no2fa@example.com", "secret", "x", false) }
        runCurrent()
        val max = events.filterIsInstance<CoreEvent.MaxCertsReached>().single()
        core.respondMaxCerts(max.certificates.mapNotNull { it.serialNumber })
        login.join()
        assertEquals("certs.no2fa@example.com", core.loggedInAs())
        assertTrue(core.certificates().isEmpty())
    }

    @Test
    fun installReportsStepsInOrder() = runTest {
        val core = core()
        val events = record(core)
        core.loginAndSelect("me.no2fa@example.com")
        core.installSidestore(nightly = false, liveContainer = false)
        val flat = events.filterIsInstance<CoreEvent.Operation>().map { it.update.updateType to it.update.stepId }
        assertEquals(
            listOf(
                UpdateType.Started to "download", UpdateType.Finished to "download",
                UpdateType.Started to "install", UpdateType.Finished to "install",
                UpdateType.Started to "pairing", UpdateType.Finished to "pairing",
            ),
            flat,
        )
    }

    @Test
    fun installFailureEmitsFailedStepAndThrows() = runTest {
        val core = core()
        val events = record(core)
        core.loginAndSelect("me.no2fa.maxapps@example.com")
        val e = assertFailsWith<IloaderException> { core.sideload("/tmp/x.ipa") }
        assertEquals("max_apps", e.type)
        val failed = events.filterIsInstance<CoreEvent.Operation>().single { it.update.updateType == UpdateType.Failed }
        assertEquals(OperationIds.SIDELOAD, failed.operationId)
        assertNotNull(failed.update.extraDetails)
    }

    @Test
    fun operationsRequireLoginAndDevice() = runTest {
        val core = core()
        assertEquals("not_logged_in", assertFailsWith<IloaderException> { core.sideload("x") }.type)
        core.loginNew("a.no2fa@example.com", "pw", "x", false)
        assertEquals("no_device_selected", assertFailsWith<IloaderException> { core.sideload("x") }.type)
    }

    @Test
    fun everyThirdDeviceRefreshIncludesAnError() = runTest {
        val core = core()
        core.listDevices()
        core.listDevices()
        assertTrue(core.listDevices().any { it is DeviceResult.Err })
    }

    @Test
    fun pairingCanBeCancelled() = runTest {
        val core = core()
        val device = (core.listDevices().first() as DeviceResult.Ok).device
        val select = launch {
            val e = assertFailsWith<IloaderException> { core.selectDevice(device) }
            assertEquals("Pairing cancelled", e.message)
        }
        runCurrent()
        core.cancelPairing()
        select.join()
    }
}
