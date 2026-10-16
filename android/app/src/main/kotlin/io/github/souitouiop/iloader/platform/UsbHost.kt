package io.github.souitouiop.iloader.platform

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbConfiguration
import android.hardware.usb.UsbConstants
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbEndpoint
import android.hardware.usb.UsbInterface
import android.hardware.usb.UsbManager
import android.os.Build
import android.util.Log
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** Where [UsbHost] hands opened devices to. Implemented by the Rust-backed core. */
interface UsbSink {
    suspend fun attachUsbDevice(
        fd: Int,
        serial: String,
        productId: Int,
        epIn: Int,
        epOut: Int,
        maxPacketOut: Int,
        name: String,
    )

    fun detachUsbDevice(name: String)
}

/**
 * Finds iPhones through [UsbManager] broadcasts, asks for permission, opens
 * the usbmux interface and gives the descriptor to the core's built-in
 * usbmuxd. Android only hands out an open file descriptor after the user
 * granted permission; the bulk transfers are done natively on it.
 */
class UsbHost(context: Context, private val sink: UsbSink) {
    private val context = context.applicationContext
    private val usb = this.context.getSystemService(Context.USB_SERVICE) as UsbManager
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /** Serializes attach and detach so a quick replug cannot interleave. */
    private val mutex = Mutex()

    /** Connections handed to the core. They have to stay open while the fd is used. */
    private val open = HashMap<String, UsbDeviceConnection>()

    /** Devices the permission dialog was already shown for. */
    private val asked = HashSet<String>()

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val device = intent.usbDevice() ?: return
            when (intent.action) {
                UsbManager.ACTION_USB_DEVICE_ATTACHED -> handle(device)
                UsbManager.ACTION_USB_DEVICE_DETACHED -> release(device.deviceName)
                ACTION_PERMISSION -> {
                    if (intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)) {
                        handle(device)
                    } else {
                        Log.i(TAG, "USB permission denied for ${device.deviceName}")
                    }
                }
            }
        }
    }

    fun start() {
        val filter = IntentFilter().apply {
            addAction(UsbManager.ACTION_USB_DEVICE_ATTACHED)
            addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
            addAction(ACTION_PERMISSION)
        }
        ContextCompat.registerReceiver(context, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
        rescan()
    }

    /** Looks at the devices that are plugged in already; call when the app comes to the front. */
    fun rescan() {
        usb.deviceList.values.filter(::isMuxCapable).forEach(::handle)
    }

    private fun handle(device: UsbDevice) {
        if (!isMuxCapable(device)) return
        if (usb.hasPermission(device)) {
            scope.launch { attach(device) }
        } else if (synchronized(asked) { asked.add(device.deviceName) }) {
            // The system fills in the result, so the intent has to be mutable.
            // Naming our own package keeps it explicit.
            val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0
            val intent = Intent(ACTION_PERMISSION).setPackage(context.packageName)
            usb.requestPermission(device, PendingIntent.getBroadcast(context, 0, intent, flags))
        }
    }

    /**
     * Opens the device, switches to the configuration that has the usbmux
     * interface if needed, claims the interface and attaches it to the core.
     */
    private suspend fun attach(device: UsbDevice) = mutex.withLock {
        val name = device.deviceName
        if (synchronized(open) { open.containsKey(name) } || usb.deviceList[name] == null) return
        val mux = findMuxInterface(device)
        if (mux == null) {
            Log.w(TAG, "$name has no usbmux interface")
            return
        }
        val connection = usb.openDevice(device)
        if (connection == null) {
            Log.w(TAG, "could not open $name")
            return
        }
        if (!isInActiveConfiguration(device, mux.usbInterface) && !connection.setConfiguration(mux.configuration)) {
            Log.w(TAG, "could not select USB configuration ${mux.configuration.id}")
            connection.close()
            return
        }
        if (!connection.claimInterface(mux.usbInterface, true)) {
            Log.w(TAG, "could not claim the usbmux interface of $name")
            connection.close()
            return
        }
        try {
            sink.attachUsbDevice(
                fd = connection.fileDescriptor,
                serial = connection.serial ?: "",
                productId = device.productId,
                epIn = mux.epIn.address,
                epOut = mux.epOut.address,
                maxPacketOut = mux.epOut.maxPacketSize,
                name = name,
            )
            synchronized(open) { open[name] = connection }
        } catch (e: Exception) {
            Log.w(TAG, "attaching $name failed", e)
            connection.close()
        }
    }

    private fun release(name: String) {
        synchronized(asked) { asked.remove(name) }
        scope.launch {
            mutex.withLock {
                sink.detachUsbDevice(name)
                synchronized(open) { open.remove(name) }?.close()
            }
        }
    }

    private class MuxInterface(
        val configuration: UsbConfiguration,
        val usbInterface: UsbInterface,
        val epIn: UsbEndpoint,
        val epOut: UsbEndpoint,
    )

    /**
     * iPhones expose several configurations and only some contain the usbmux
     * interface. Like usbmuxd, look from the last one down.
     */
    private fun findMuxInterface(device: UsbDevice): MuxInterface? {
        for (c in device.configurationCount - 1 downTo 0) {
            val configuration = device.getConfiguration(c)
            for (i in 0 until configuration.interfaceCount) {
                val intf = configuration.getInterface(i)
                if (intf.interfaceClass != INTERFACE_CLASS ||
                    intf.interfaceSubclass != INTERFACE_SUBCLASS ||
                    intf.interfaceProtocol != INTERFACE_PROTOCOL ||
                    intf.endpointCount != 2
                ) {
                    continue
                }
                var epIn: UsbEndpoint? = null
                var epOut: UsbEndpoint? = null
                for (e in 0 until intf.endpointCount) {
                    val endpoint = intf.getEndpoint(e)
                    if (endpoint.type != UsbConstants.USB_ENDPOINT_XFER_BULK) continue
                    if (endpoint.direction == UsbConstants.USB_DIR_IN) epIn = endpoint else epOut = endpoint
                }
                if (epIn != null && epOut != null) {
                    return MuxInterface(configuration, intf, epIn, epOut)
                }
            }
        }
        return null
    }

    /** `UsbDevice.getInterface` only lists the interfaces of the active configuration. */
    private fun isInActiveConfiguration(device: UsbDevice, wanted: UsbInterface): Boolean {
        for (i in 0 until device.interfaceCount) {
            val active = device.getInterface(i)
            if (active.id == wanted.id &&
                active.alternateSetting == wanted.alternateSetting &&
                active.interfaceClass == wanted.interfaceClass &&
                active.interfaceSubclass == wanted.interfaceSubclass &&
                active.interfaceProtocol == wanted.interfaceProtocol
            ) {
                return true
            }
        }
        return false
    }

    /** Same product id range usbmuxd accepts. */
    private fun isMuxCapable(device: UsbDevice): Boolean {
        if (device.vendorId != VENDOR_APPLE) return false
        return device.productId in 0x1290..0x12af || device.productId in 0x1901..0x1905
    }

    private fun Intent.usbDevice(): UsbDevice? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
        } else {
            @Suppress("DEPRECATION")
            getParcelableExtra(UsbManager.EXTRA_DEVICE)
        }

    private companion object {
        const val TAG = "iloader-usb"
        const val ACTION_PERMISSION = "io.github.souitouiop.iloader.USB_PERMISSION"
        const val VENDOR_APPLE = 0x05ac
        const val INTERFACE_CLASS = 255
        const val INTERFACE_SUBCLASS = 254
        const val INTERFACE_PROTOCOL = 2
    }
}
