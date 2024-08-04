#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::{
    clock::ClockControl,
    delay::Delay,
    gpio::{Input, Io, Level, Output, Pull},
    peripherals::Peripherals,
    prelude::*,
    rng::Rng,
    system::SystemControl,
    // timer::{systimer::SystemTimer, PeriodicTimer},
    timer::{timg::TimerGroup, ErasedTimer, PeriodicTimer},
};

use embedded_io::*;
use esp_wifi::wifi::{AccessPointInfo, AuthMethod, ClientConfiguration, Configuration};

use esp_println::{print, println};
use esp_wifi::wifi::utils::create_network_interface;
use esp_wifi::wifi::{WifiError, WifiStaDevice};
use esp_wifi::wifi_interface::WifiStack;
use esp_wifi::{current_millis, initialize, EspWifiInitFor};
use smoltcp::iface::SocketStorage;
use smoltcp::wire::IpAddress;
use smoltcp::wire::Ipv4Address;

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

#[entry]
fn main() -> ! {
    let peripherals = Peripherals::take();
    let system = SystemControl::new(peripherals.SYSTEM);

    let clocks = ClockControl::max(system.clock_control).freeze();
    let delay = Delay::new(&clocks);

    esp_println::logger::init_logger_from_env();

    let timg0 = TimerGroup::new(peripherals.TIMG0, &clocks, None);
    let timer0: ErasedTimer = timg0.timer0.into();
    let timer = PeriodicTimer::new(timer0);

    let mut wdt0 = timg0.wdt;
    wdt0.enable();
    wdt0.set_timeout(60u64.secs());

    let io = Io::new(peripherals.GPIO, peripherals.IO_MUX);
    let mut led = Output::new(io.pins.gpio23, Level::Low);
    let button = Input::new(io.pins.gpio36, Pull::None);

    let init = initialize(
        EspWifiInitFor::Wifi,
        timer,
        Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
        &clocks,
    )
    .unwrap();

    wdt0.feed();

    let wifi = peripherals.WIFI;
    let mut socket_set_entries: [SocketStorage; 3] = Default::default();
    let (iface, device, mut controller, sockets) =
        create_network_interface(&init, wifi, WifiStaDevice, &mut socket_set_entries).unwrap();
    let wifi_stack = WifiStack::new(iface, device, sockets, current_millis);

    let client_config = Configuration::Client(ClientConfiguration {
        ssid: SSID.try_into().unwrap(),
        password: PASSWORD.try_into().unwrap(),
        ..Default::default()
    });
    let res = controller.set_configuration(&client_config);
    println!("wifi_set_configuration returned {:?}", res);

    wdt0.feed();

    controller.start().unwrap();
    println!("is wifi started: {:?}", controller.is_started());

    println!("Start Wifi Scan");
    let res: Result<(heapless::Vec<AccessPointInfo, 10>, usize), WifiError> = controller.scan_n();
    if let Ok((res, _count)) = res {
        for ap in res {
            println!("{:?}", ap);
        }
    }

    wdt0.feed();

    println!("{:?}", controller.get_capabilities());
    println!("wifi_connect {:?}", controller.connect());

    // wait to get connected
    println!("Wait to get connected");
    loop {
        wdt0.feed();
        let res = controller.is_connected();
        match res {
            Ok(connected) => {
                if connected {
                    break;
                }
            }
            Err(err) => {
                println!("{:?}", err);
                loop {}
            }
        }
    }
    println!("{:?}", controller.is_connected());

    // wait for getting an ip address
    println!("Wait to get an ip address");
    loop {
        wdt0.feed();
        wifi_stack.work();

        if wifi_stack.is_iface_up() {
            println!("got ip {:?}", wifi_stack.get_ip_info());
            break;
        }
    }

    println!("Start busy loop on main");
    let mut rx_buffer = [0u8; 1536];
    let mut tx_buffer = [0u8; 1536];
    let mut socket = wifi_stack.get_socket(&mut rx_buffer, &mut tx_buffer);

    // 192.168.1.5/kettle.php?str=washing
    // washing urlnencoded

    let mut wm_started = false;
    let mut led_wait_end = current_millis() + 500;
    // Check the button state and set the LED state accordingly.
    loop {
        wdt0.feed();
        socket.work();
        // при нажатии кнопки уровень 0 и ставим флаг что машина запущена
        if button.is_low() && wm_started == false {
            wm_started = true;
            log::info!("WM Started!");
        }
        // машина была запущена и остановилась - отправляем письмо
        if button.is_high() && wm_started {
            wdt0.feed();
            wm_started = false;
            // send email
            log::info!("EMail sending...");
            socket
                .open(IpAddress::Ipv4(Ipv4Address::new(192, 168, 1, 5)), 80)
                .unwrap();
            socket
                .write(b"GET /kettle.php?str=washStopped HTTP/1.0\r\n\r\n")
                .unwrap();
            socket.flush().unwrap();
            wdt0.feed();

            let wait_end = current_millis() + 20 * 1000;
            loop {
                wdt0.feed();
                let mut buffer = [0u8; 512];
                if let Ok(len) = socket.read(&mut buffer) {
                    let to_print = unsafe { core::str::from_utf8_unchecked(&buffer[..len]) };
                    print!("{}", to_print);
                } else {
                    break;
                }

                if current_millis() > wait_end {
                    println!("Timeout");
                    break;
                }
            }
            println!();

            socket.disconnect();
        }
        // пробуем каждые полсекунды мигать диодом - heartbeat
        if current_millis() > led_wait_end {
            led.toggle();
            led_wait_end = current_millis() + 500;
        }
    }
}
