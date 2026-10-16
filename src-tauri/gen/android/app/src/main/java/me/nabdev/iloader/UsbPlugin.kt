package me.nabdev.iloader

import android.app.Activity
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
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class UsbDeviceArgs {
    lateinit var name: String
}

/**
 * Gives the Rust side access to the usbmux interface of an iPhone.
 *
 * Android only lets an app open a USB device after the user granted it
 * permission, and then only hands out an open file descriptor. This plugin
 * does the discovery, permission and interface setup; the actual bulk
 * transfers are done natively on the descriptor (see usbfs.rs).
 */
@TauriPlugin
class UsbPlugin(private val activity: Activity) : Plugin(activity) {
    private val usb = activity.getSystemService(Context.USB_SERVICE) as UsbManager

    /** Connections handed to Rust. They have to stay open while the fd is used. */
    private val open = HashMap<String, UsbDeviceConnection>()

    /** Callers waiting for the user to answer the permission dialog. */
    private val pending = HashMap<String, MutableList<Invoke>>()

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action != ACTION_PERMISSION) return
            val device: UsbDevice? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableExtra(UsbManager.EXTRA_DEVICE)
            }
            val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
            val name = device?.deviceName ?: return
            val waiting = synchronized(pending) { pending.remove(name) } ?: return
            for (invoke in waiting) {
                invoke.resolve(JSObject().put("granted", granted))
            }
        }
    }

    init {
        ContextCompat.registerReceiver(
            activity,
            receiver,
            IntentFilter(ACTION_PERMISSION),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
    }

    @Command
    fun listDevices(invoke: Invoke) {
        val devices = JSArray()
        for (device in usb.deviceList.values) {
            if (!isMuxCapable(device)) continue
            devices.put(
                JSObject()
                    .put("name", device.deviceName)
                    .put("vendorId", device.vendorId)
                    .put("productId", device.productId)
                    .put("hasPermission", usb.hasPermission(device)),
            )
        }
        invoke.resolve(JSObject().put("devices", devices))
    }

    @Command
    fun requestPermission(invoke: Invoke) {
        val args = invoke.parseArgs(UsbDeviceArgs::class.java)
        val device = usb.deviceList[args.name]
            ?: return invoke.reject("USB device ${args.name} is not attached")
        if (usb.hasPermission(device)) {
            return invoke.resolve(JSObject().put("granted", true))
        }
        synchronized(pending) {
            pending.getOrPut(args.name) { mutableListOf() }.add(invoke)
        }
        // The system fills in the result, so the intent has to be mutable.
        // Naming our own package keeps it explicit.
        val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0
        val intent = Intent(ACTION_PERMISSION).setPackage(activity.packageName)
        usb.requestPermission(device, PendingIntent.getBroadcast(activity, 0, intent, flags))
    }

    /**
     * Opens the device, switches to the configuration that has the usbmux
     * interface if needed, claims the interface and returns the file
     * descriptor together with the endpoints to use.
     */
    @Command
    fun openDevice(invoke: Invoke) {
        val args = invoke.parseArgs(UsbDeviceArgs::class.java)
        val device = usb.deviceList[args.name]
            ?: return invoke.reject("USB device ${args.name} is not attached")
        if (!usb.hasPermission(device)) {
            return invoke.reject("No permission to use ${args.name}")
        }
        val mux = findMuxInterface(device)
            ?: return invoke.reject("The device has no usbmux interface")

        closeConnection(args.name)
        val connection = usb.openDevice(device)
            ?: return invoke.reject("Could not open ${args.name}")

        if (!isInActiveConfiguration(device, mux.usbInterface) &&
            !connection.setConfiguration(mux.configuration)
        ) {
            connection.close()
            return invoke.reject("Could not select USB configuration ${mux.configuration.id}")
        }
        if (!connection.claimInterface(mux.usbInterface, true)) {
            connection.close()
            return invoke.reject("Could not claim the usbmux interface")
        }

        synchronized(open) { open[args.name] = connection }
        invoke.resolve(
            JSObject()
                .put("fd", connection.fileDescriptor)
                .put("serial", connection.serial ?: "")
                .put("productId", device.productId)
                .put("interfaceId", mux.usbInterface.id)
                .put("epIn", mux.epIn.address)
                .put("epOut", mux.epOut.address)
                .put("maxPacketOut", mux.epOut.maxPacketSize),
        )
    }

    @Command
    fun closeDevice(invoke: Invoke) {
        val args = invoke.parseArgs(UsbDeviceArgs::class.java)
        closeConnection(args.name)
        invoke.resolve()
    }

    private fun closeConnection(name: String) {
        val connection = synchronized(open) { open.remove(name) } ?: return
        connection.close()
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

    companion object {
        private const val ACTION_PERMISSION = "me.nabdev.iloader.USB_PERMISSION"
        private const val VENDOR_APPLE = 0x05ac
        private const val INTERFACE_CLASS = 255
        private const val INTERFACE_SUBCLASS = 254
        private const val INTERFACE_PROTOCOL = 2
    }
}
