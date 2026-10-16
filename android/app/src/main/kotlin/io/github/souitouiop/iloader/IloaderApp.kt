package io.github.souitouiop.iloader

import android.app.Application
import io.github.souitouiop.iloader.core.FakeCore
import io.github.souitouiop.iloader.core.IloaderCore
import io.github.souitouiop.iloader.core.RealCore
import io.github.souitouiop.iloader.data.Prefs
import io.github.souitouiop.iloader.platform.UsbHost

class IloaderApp : Application() {
    lateinit var prefs: Prefs
        private set

    /** The backend: the Rust core, or [FakeCore] when built with `-PfakeCore=true`. */
    lateinit var core: IloaderCore
        private set

    /** Null with [FakeCore]. */
    var usbHost: UsbHost? = null
        private set

    override fun onCreate() {
        super.onCreate()
        prefs = Prefs(this)
        if (BuildConfig.USE_FAKE_CORE) {
            core = FakeCore()
        } else {
            val real = RealCore(this)
            core = real
            usbHost = UsbHost(this, real).also { it.start() }
        }
    }
}
