// **************************************************************************
// Copyright (c) 2015 Osspial All Rights Reserved.
//
// This file is part of hidapi-rs, based on hidapi_rust by Roland Ruckerbauer.
// *************************************************************************

//! This crate provides a rust abstraction over the features of the C library
//! hidapi by [signal11](https://github.com/libusb/hidapi).
//!
//! # Usage
//!
//! This crate is [on crates.io](https://crates.io/crates/hidapi) and can be
//! used by adding `hidapi` to the dependencies in your project's `Cargo.toml`.
//!
//! # Example
//!
//! ```rust,no_run
//! extern crate hidapi;
//!
//! use hidapi::HidApi;
//!
//! fn main() {
//!     println!("Printing all available hid devices:");
//!
//!     match HidApi::new() {
//!         Ok(api) => {
//!             for device in api.device_list() {
//!                 println!("{:04x}:{:04x}", device.vendor_id(), device.product_id());
//!             }
//!         },
//!         Err(e) => {
//!             eprintln!("Error: {}", e);
//!         },
//!     }
//! }
//! ```
//!
//! # Feature flags
//!
//! - `linux-static-libusb`: uses statically linked `libusb` backend on Linux
//! - `linux-static-hidraw`: uses statically linked `hidraw` backend on Linux (default)
//! - `linux-shared-libusb`: uses dynamically linked `libusb` backend on Linux
//! - `linux-shared-hidraw`: uses dynamically linked `hidraw` backend on Linux
//! - `linux-native`: talks to hidraw directly without using the `hidapi` C library
//! - `illumos-static-libusb`: uses statically linked `libusb` backend on Illumos (default)
//! - `illumos-shared-libusb`: uses statically linked `hidraw` backend on Illumos
//! - `macos-shared-device`: enables shared access to HID devices on MacOS
//!
//! ## Linux backends
//!
//! On linux the libusb backends do not support [`DeviceInfo::usage()`] and [`DeviceInfo::usage_page()`].
//! The hidraw backend has support for them, but it might be buggy in older kernel versions.
//!
//! ## MacOS Shared device access
//!
//! Since `hidapi` 0.12 it is possible to open MacOS devices with shared access, so that multiple
//! [`HidDevice`] handles can access the same physical device. For backward compatibility this is
//! an opt-in that can be enabled with the `macos-shared-device` feature flag.

extern crate libc;
#[cfg(feature = "linux-native")]
extern crate nix;

#[cfg(target_os = "windows")]
extern crate winapi;
#[cfg(target_os = "windows")]
use winapi::shared::guiddef::GUID;

mod error;
mod ffi;

#[cfg(hidapi)]
mod hidapi;

#[cfg(feature = "linux-native")]
#[cfg_attr(docsrs, doc(cfg(feature = "linux-native")))]
mod linux_native;
#[cfg(target_os = "macos")]
#[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
mod macos;
#[cfg(target_os = "windows")]
#[cfg_attr(docsrs, doc(cfg(target_os = "windows")))]
mod windows;

use libc::wchar_t;
use std::ffi::CStr;
use std::ffi::CString;
use std::fmt;
use std::fmt::Debug;
use std::sync::Mutex;

pub use error::HidError;

#[cfg(hidapi)]
use crate::hidapi::HidApiBackend;
#[cfg(feature = "linux-native")]
use linux_native::HidApiBackend;

pub type HidResult<T> = Result<T, HidError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitState {
    NotInit,
    Init { enumerate: bool },
}

static INIT_STATE: Mutex<InitState> = Mutex::new(InitState::NotInit);

fn lazy_init(do_enumerate: bool) -> HidResult<()> {
    let mut init_state = INIT_STATE.lock().unwrap();

    match *init_state {
        InitState::NotInit => {
            #[cfg(all(libusb, not(target_os = "freebsd")))]
            if !do_enumerate {
                // Do not scan for devices in libusb_init()
                // Must be set before calling it.
                // This is needed on Android, where access to USB devices is limited
                unsafe { ffi::libusb_set_option(std::ptr::null_mut(), 2) }
            }

            // Initialize the HID
            #[cfg(hidapi)]
            if unsafe { ffi::hid_init() } == -1 {
                return Err(HidError::InitializationError);
            }

            #[cfg(all(target_os = "macos", feature = "macos-shared-device"))]
            unsafe {
                ffi::macos::hid_darwin_set_open_exclusive(0)
            }

            *init_state = InitState::Init {
                enumerate: do_enumerate,
            }
        }
        InitState::Init { enumerate } => {
            if enumerate != do_enumerate {
                panic!("Trying to initialize hidapi with enumeration={}, but it is already initialized with enumeration={}.", do_enumerate, enumerate)
            }
        }
    }

    Ok(())
}

/// `hidapi` context.
///
/// The `hidapi` C library is lazily initialized when creating the first instance,
/// and never deinitialized. Therefore, it is allowed to create multiple `HidApi`
/// instances.
///
/// Each instance has its own device list cache.
pub struct HidApi {
    device_list: Vec<DeviceInfo>,
}

impl HidApi {
    /// Create a new hidapi context.
    ///
    /// Will also initialize the currently available device list.
    ///
    /// # Panics
    ///
    /// Panics if hidapi is already initialized in "without enumerate" mode
    /// (i.e. if `new_without_enumerate()` has been called before).
    pub fn new() -> HidResult<Self> {
        lazy_init(true)?;

        let device_list = HidApiBackend::get_hid_device_info_vector()?;

        Ok(HidApi { device_list })
    }

    /// Create a new hidapi context, in "do not enumerate" mode.
    ///
    /// This is needed on Android, where access to USB device enumeration is limited.
    ///
    /// # Panics
    ///
    /// Panics if hidapi is already initialized in "do enumerate" mode
    /// (i.e. if `new()` has been called before).
    pub fn new_without_enumerate() -> HidResult<Self> {
        lazy_init(false)?;

        Ok(HidApi {
            device_list: Vec::new(),
        })
    }

    /// Refresh devices list and information about them (to access them use
    /// `device_list()` method)
    pub fn refresh_devices(&mut self) -> HidResult<()> {
        let device_list = HidApiBackend::get_hid_device_info_vector()?;
        self.device_list = device_list;
        Ok(())
    }

    /// Returns iterator containing information about attached HID devices.
    pub fn device_list(&self) -> impl Iterator<Item = &DeviceInfo> {
        self.device_list.iter()
    }

    /// Open a HID device using a Vendor ID (VID) and Product ID (PID).
    ///
    /// When multiple devices with the same vid and pid are available, then the
    /// first one found in the internal device list will be used. There are however
    /// no guarantees, which device this will be.
    pub fn open(&self, vid: u16, pid: u16) -> HidResult<HidDevice> {
        let dev = HidApiBackend::open(vid, pid)?;
        Ok(HidDevice::from_backend(Box::new(dev)))
    }

    /// Open a HID device using a Vendor ID (VID), Product ID (PID) and
    /// a serial number.
    pub fn open_serial(&self, vid: u16, pid: u16, sn: &str) -> HidResult<HidDevice> {
        let dev = HidApiBackend::open_serial(vid, pid, sn)?;
        Ok(HidDevice::from_backend(Box::new(dev)))
    }

    /// The path name be determined by inspecting the device list available with [HidApi::devices()](struct.HidApi.html#method.devices)
    ///
    /// Alternatively a platform-specific path name can be used (eg: /dev/hidraw0 on Linux).
    pub fn open_path(&self, device_path: &CStr) -> HidResult<HidDevice> {
        let dev = HidApiBackend::open_path(device_path)?;
        Ok(HidDevice::from_backend(Box::new(dev)))
    }

    /// Open a HID device using libusb_wrap_sys_device.
    #[cfg(libusb)]
    pub fn wrap_sys_device(&self, sys_dev: isize, interface_num: i32) -> HidResult<HidDevice> {
        let device = unsafe { ffi::hid_libusb_wrap_sys_device(sys_dev, interface_num) };

        if device.is_null() {
            match self.check_error() {
                Ok(err) => Err(err),
                Err(e) => Err(e),
            }
        } else {
            let dev = hidapi::HidDevice::from_raw(device);
            Ok(HidDevice::from_backend(Box::new(dev)))
        }
    }

    /// Get the last non-device specific error, which happened in the underlying hidapi C library.
    /// To get the last device specific error, use [`HidDevice::check_error`].
    ///
    /// The `Ok()` variant of the result will contain a [HidError::HidApiError](enum.HidError.html).
    ///
    /// When `Err()` is returned, then acquiring the error string from the hidapi C
    /// library failed. The contained [HidError](enum.HidError.html) is the cause, why no error could
    /// be fetched.
    #[cfg(hidapi)]
    #[deprecated(since = "2.2.3", note = "use the return values from the other methods")]
    pub fn check_error(&self) -> HidResult<HidError> {
        HidApiBackend::check_error()
    }
}

#[derive(Clone, PartialEq)]
enum WcharString {
    String(String),
    #[cfg_attr(feature = "linux-native", allow(dead_code))]
    Raw(Vec<wchar_t>),
    None,
}

impl From<WcharString> for Option<String> {
    fn from(val: WcharString) -> Self {
        match val {
            WcharString::String(s) => Some(s),
            _ => None,
        }
    }
}

/// The underlying HID bus type.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub enum BusType {
    Unknown = 0x00,
    Usb = 0x01,
    Bluetooth = 0x02,
    I2c = 0x03,
    Spi = 0x04,
}

/// Device information. Use accessors to extract information about Hid devices.
///
/// Note: Methods like `serial_number()` may return None, if the conversion to a
/// String failed internally. You can however access the raw hid representation of the
/// string by calling `serial_number_raw()`
#[derive(Clone)]
pub struct DeviceInfo {
    path: CString,
    vendor_id: u16,
    product_id: u16,
    serial_number: WcharString,
    release_number: u16,
    manufacturer_string: WcharString,
    product_string: WcharString,
    #[allow(dead_code)]
    usage_page: u16,
    #[allow(dead_code)]
    usage: u16,
    interface_number: i32,
    bus_type: BusType,
}

impl DeviceInfo {
    pub fn path(&self) -> &CStr {
        &self.path
    }

    pub fn vendor_id(&self) -> u16 {
        self.vendor_id
    }

    pub fn product_id(&self) -> u16 {
        self.product_id
    }

    /// Try to call `serial_number_raw()`, if None is returned.
    pub fn serial_number(&self) -> Option<&str> {
        match self.serial_number {
            WcharString::String(ref s) => Some(s),
            _ => None,
        }
    }

    pub fn serial_number_raw(&self) -> Option<&[wchar_t]> {
        match self.serial_number {
            WcharString::Raw(ref s) => Some(s),
            _ => None,
        }
    }

    pub fn release_number(&self) -> u16 {
        self.release_number
    }

    /// Try to call `manufacturer_string_raw()`, if None is returned.
    pub fn manufacturer_string(&self) -> Option<&str> {
        match self.manufacturer_string {
            WcharString::String(ref s) => Some(s),
            _ => None,
        }
    }

    pub fn manufacturer_string_raw(&self) -> Option<&[wchar_t]> {
        match self.manufacturer_string {
            WcharString::Raw(ref s) => Some(s),
            _ => None,
        }
    }

    /// Try to call `product_string_raw()`, if None is returned.
    pub fn product_string(&self) -> Option<&str> {
        match self.product_string {
            WcharString::String(ref s) => Some(s),
            _ => None,
        }
    }

    pub fn product_string_raw(&self) -> Option<&[wchar_t]> {
        match self.product_string {
            WcharString::Raw(ref s) => Some(s),
            _ => None,
        }
    }

    /// Usage page is not available on linux libusb backends
    #[cfg(not(all(libusb, target_os = "linux")))]
    pub fn usage_page(&self) -> u16 {
        self.usage_page
    }

    /// Usage is not available on linux libusb backends
    #[cfg(not(all(libusb, target_os = "linux")))]
    pub fn usage(&self) -> u16 {
        self.usage
    }

    pub fn interface_number(&self) -> i32 {
        self.interface_number
    }

    pub fn bus_type(&self) -> BusType {
        self.bus_type
    }

    /// Use the information contained in `DeviceInfo` to open
    /// and return a handle to a [HidDevice](struct.HidDevice.html).
    ///
    /// By default the device path is used to open the device.
    /// When no path is available, then vid, pid and serial number are used.
    /// If both path and serial number are not available, then this function will
    /// fail with [HidError::OpenHidDeviceWithDeviceInfoError](enum.HidError.html#variant.OpenHidDeviceWithDeviceInfoError).
    ///
    /// Note, that opening a device could still be done using [HidApi::open()](struct.HidApi.html#method.open) directly.
    pub fn open_device(&self, hidapi: &HidApi) -> HidResult<HidDevice> {
        if !self.path.as_bytes().is_empty() {
            hidapi.open_path(self.path.as_c_str())
        } else if let Some(sn) = self.serial_number() {
            hidapi.open_serial(self.vendor_id, self.product_id, sn)
        } else {
            Err(HidError::OpenHidDeviceWithDeviceInfoError {
                device_info: Box::new(self.clone()),
            })
        }
    }
}

impl fmt::Debug for DeviceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HidDeviceInfo")
            .field("vendor_id", &self.vendor_id)
            .field("product_id", &self.product_id)
            .finish()
    }
}

/// Trait which the different backends must implement
trait HidDeviceBackendBase {
    #[cfg(hidapi)]
    fn check_error(&self) -> HidResult<HidError>;
    fn write(&self, data: &[u8]) -> HidResult<usize>;
    fn read(&self, buf: &mut [u8]) -> HidResult<usize>;
    fn read_timeout(&self, buf: &mut [u8], timeout: i32) -> HidResult<usize>;
    fn send_feature_report(&self, data: &[u8]) -> HidResult<()>;
    fn get_feature_report(&self, buf: &mut [u8]) -> HidResult<usize>;
    fn set_blocking_mode(&self, blocking: bool) -> HidResult<()>;
    fn get_device_info(&self) -> HidResult<DeviceInfo>;
    fn get_manufacturer_string(&self) -> HidResult<Option<String>>;
    fn get_product_string(&self) -> HidResult<Option<String>>;
    fn get_serial_number_string(&self) -> HidResult<Option<String>>;

    fn get_indexed_string(&self, _index: i32) -> HidResult<Option<String>> {
        Err(HidError::HidApiError {
            message: "get_indexed_string: not supported".to_string(),
        })
    }
}

/// A trait with the extra methods that are available on macOS
#[cfg(target_os = "macos")]
trait HidDeviceBackendMacos {
    /// Get the location ID for a [`HidDevice`] device.
    fn get_location_id(&self) -> HidResult<u32>;

    /// Check if the device was opened in exclusive mode.
    fn is_open_exclusive(&self) -> HidResult<bool>;
}

/// A trait with the extra methods that are available on macOS
#[cfg(target_os = "windows")]
trait HidDeviceBackendWindows {
    /// Get the container ID for a HID device
    fn get_container_id(&self) -> HidResult<GUID>;
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
trait HidDeviceBackend: HidDeviceBackendBase + Send {}
#[cfg(target_os = "macos")]
trait HidDeviceBackend: HidDeviceBackendBase + HidDeviceBackendMacos + Send {}
#[cfg(target_os = "windows")]
trait HidDeviceBackend: HidDeviceBackendBase + HidDeviceBackendWindows + Send {}

/// Automatically implement the top trait
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl<T> HidDeviceBackend for T where T: HidDeviceBackendBase + Send {}

/// Automatically implement the top trait
#[cfg(target_os = "macos")]
impl<T> HidDeviceBackend for T where T: HidDeviceBackendBase + HidDeviceBackendMacos + Send {}

/// Automatically implement the top trait
#[cfg(target_os = "windows")]
impl<T> HidDeviceBackend for T where T: HidDeviceBackendBase + HidDeviceBackendWindows + Send {}

pub struct HidDevice {
    inner: Box<dyn HidDeviceBackend>,
}

impl Debug for HidDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HidDevice").finish_non_exhaustive()
    }
}

impl HidDevice {
    fn from_backend(inner: Box<dyn HidDeviceBackend>) -> Self {
        Self { inner }
    }
}

// Methods that use the backend
impl HidDevice {
    /// Get the last error, which happened in the underlying hidapi C library.
    ///
    /// The `Ok()` variant of the result will contain a [HidError::HidApiError](enum.HidError.html).
    ///
    /// When `Err()` is returned, then acquiring the error string from the hidapi C
    /// library failed. The contained [HidError](enum.HidError.html) is the cause, why no error could
    /// be fetched.
    #[cfg(hidapi)]
    #[deprecated(since = "2.2.3", note = "use the return values from the other methods")]
    pub fn check_error(&self) -> HidResult<HidError> {
        self.inner.check_error()
    }

    /// The first byte of `data` must contain the Report ID. For
    /// devices which only support a single report, this must be set
    /// to 0x0. The remaining bytes contain the report data. Since
    /// the Report ID is mandatory, calls to `write()` will always
    /// contain one more byte than the report contains. For example,
    /// if a hid report is 16 bytes long, 17 bytes must be passed to
    /// `write()`, the Report ID (or 0x0, for devices with a
    /// single report), followed by the report data (16 bytes). In
    /// this example, the length passed in would be 17.
    /// `write()` will send the data on the first OUT endpoint, if
    /// one exists. If it does not, it will send the data through
    /// the Control Endpoint (Endpoint 0).
    pub fn write(&self, data: &[u8]) -> HidResult<usize> {
        self.inner.write(data)
    }

    /// Input reports are returned to the host through the 'INTERRUPT IN'
    /// endpoint. The first byte will contain the Report number if the device
    /// uses numbered reports.
    pub fn read(&self, buf: &mut [u8]) -> HidResult<usize> {
        self.inner.read(buf)
    }

    /// Input reports are returned to the host through the 'INTERRUPT IN'
    /// endpoint. The first byte will contain the Report number if the device
    /// uses numbered reports. Timeout measured in milliseconds, set -1 for
    /// blocking wait.
    pub fn read_timeout(&self, buf: &mut [u8], timeout: i32) -> HidResult<usize> {
        self.inner.read_timeout(buf, timeout)
    }

    /// Send a Feature report to the device.
    /// Feature reports are sent over the Control endpoint as a
    /// Set_Report transfer.  The first byte of `data` must contain the
    /// 'Report ID'. For devices which only support a single report, this must
    /// be set to 0x0. The remaining bytes contain the report data. Since the
    /// 'Report ID' is mandatory, calls to `send_feature_report()` will always
    /// contain one more byte than the report contains. For example, if a hid
    /// report is 16 bytes long, 17 bytes must be passed to
    /// `send_feature_report()`: 'the Report ID' (or 0x0, for devices which
    /// do not use numbered reports), followed by the report data (16 bytes).
    /// In this example, the length passed in would be 17.
    pub fn send_feature_report(&self, data: &[u8]) -> HidResult<()> {
        self.inner.send_feature_report(data)
    }

    /// Set the first byte of `buf` to the 'Report ID' of the report to be read.
    /// Upon return, the first byte will still contain the Report ID, and the
    /// report data will start in `buf[1]`.
    pub fn get_feature_report(&self, buf: &mut [u8]) -> HidResult<usize> {
        self.inner.get_feature_report(buf)
    }

    /// Set the device handle to be in blocking or in non-blocking mode. In
    /// non-blocking mode calls to `read()` will return immediately with an empty
    /// slice if there is no data to be read. In blocking mode, `read()` will
    /// wait (block) until there is data to read before returning.
    /// Modes can be changed at any time.
    pub fn set_blocking_mode(&self, blocking: bool) -> HidResult<()> {
        self.inner.set_blocking_mode(blocking)
    }

    /// Get The Manufacturer String from a HID device.
    pub fn get_manufacturer_string(&self) -> HidResult<Option<String>> {
        self.inner.get_manufacturer_string()
    }

    /// Get The Manufacturer String from a HID device.
    pub fn get_product_string(&self) -> HidResult<Option<String>> {
        self.inner.get_product_string()
    }

    /// Get The Serial Number String from a HID device.
    pub fn get_serial_number_string(&self) -> HidResult<Option<String>> {
        self.inner.get_serial_number_string()
    }

    /// Get a string from a HID device, based on its string index.
    pub fn get_indexed_string(&self, index: i32) -> HidResult<Option<String>> {
        self.inner.get_indexed_string(index)
    }

    /// Get [`DeviceInfo`] from a HID device.
    pub fn get_device_info(&self) -> HidResult<DeviceInfo> {
        self.inner.get_device_info()
    }
}
