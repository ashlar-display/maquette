//! The async traits on the bench's handles: hdmi-hal-async's `Send` twins, the plain traits
//! through its blanket implementations, and futures that are `Send`.

#![cfg(feature = "async")]

use core::future::Future;
use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::hdmi_hal_async::ddc::{DdcBus, SendDdcBus};
use maquette::hdmi_hal_async::phy::{FrlPhy, SendFrlPhy, SendPhyPower, SendTmdsPhy, TmdsPhy};
use maquette::hdmi_hal_async::scdc::{SendScdcOverDdc, SendScdcTransport};
use maquette::hdmi_hal_async::time::{Delay, SendDelay};
use maquette::{
    Bench, Clock, ColorBitDepth, ColorFormat, EddcTarget, HdmiForumFrl, Line, SinkConfig,
    TmdsClockRatio, TmdsPhyConfig, TmdsVideo,
};

fn high() -> TmdsPhyConfig {
    let video = TmdsVideo::new(594_000, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    TmdsPhyConfig::new(594_000, TmdsClockRatio::Div40, video)
}

fn assert_send<T: Send>(value: T) -> T {
    value
}

/// A bring-up over the `Send` twins alone, as a procedure spawned on a multi-threaded
/// executor is bounded: the compiler proves its future `Send` for every such part.
#[expect(
    clippy::manual_async_fn,
    reason = "the return type states that the future is Send: that is the probe"
)]
fn bring_up<'f, B, P, C, D>(
    bus: &'f B,
    phy: &'f mut P,
    clock: &'f C,
    delay: &'f mut D,
) -> impl Future<Output = Result<u8, ()>> + Send + 'f
where
    B: SendDdcBus,
    P: SendTmdsPhy + SendPhyPower,
    C: Clock + Sync,
    D: SendDelay,
{
    async move {
        let start = clock.now();
        SendDdcBus::write(bus, SCDC, &[0x20, 0x03])
            .await
            .map_err(drop)?;
        SendPhyPower::power_on(phy).await.map_err(drop)?;
        SendTmdsPhy::configure(phy, high()).await.map_err(drop)?;
        SendTmdsPhy::set_scrambling(phy, true).await.map_err(drop)?;
        SendTmdsPhy::start_output(phy).await.map_err(drop)?;
        SendDelay::delay(delay, Duration::from_millis(1)).await;
        let mut status = [0];
        SendDdcBus::write_read(bus, SCDC, &[0x21], &mut status)
            .await
            .map_err(drop)?;
        assert!(clock.now() > start);
        Ok(status[0])
    }
}

#[test]
fn an_async_bring_up_over_the_send_twins_locks_the_sink() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let bus = bench.ddc();
    let mut phy = bench.phy();
    let time = bench.time();
    let mut delay = bench.time();
    let future = assert_send(bring_up(&bus, &mut phy, &time, &mut delay));
    assert_eq!(pollster::block_on(future), Ok(1));
    assert_eq!(
        bench.line(),
        Line::Tmds {
            character_rate_khz: 594_000,
            ratio: TmdsClockRatio::Div40,
            scrambled: true,
        }
    );
}

#[test]
fn the_future_runs_on_another_thread() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let bus = bench.ddc();
    let mut phy = bench.phy();
    let time = bench.time();
    let mut delay = bench.time();
    let future = bring_up(&bus, &mut phy, &time, &mut delay);
    let status = std::thread::scope(|s| s.spawn(move || pollster::block_on(future)).join());
    assert_eq!(status.unwrap(), Ok(1));
}

#[test]
fn the_plain_async_traits_come_through_the_blanket_implementations() {
    async fn plain<B: DdcBus, P: TmdsPhy + FrlPhy, D: Delay>(
        bus: &B,
        phy: &mut P,
        delay: &mut D,
    ) -> bool {
        let mut buf = [0u8; 2];
        let edid = bus
            .read_segmented(EddcTarget::Edid, 0, 0, &mut buf)
            .await
            .is_ok();
        let tmds = TmdsPhy::configure(phy, high()).await.is_ok();
        let frl = FrlPhy::set_frl_rate(phy, HdmiForumFrl::Rate3Gbps3Lanes)
            .await
            .is_ok();
        delay.delay(Duration::from_micros(10)).await;
        edid && tmds && frl
    }
    let edid = [0u8; 128];
    let bench = Bench::new(SinkConfig::new(), &edid).unwrap();
    let mut phy = bench.phy();
    let mut delay = bench.time();
    assert!(pollster::block_on(plain(
        &bench.ddc(),
        &mut phy,
        &mut delay
    )));
    assert_eq!(bench.line(), Line::frl(HdmiForumFrl::Rate3Gbps3Lanes));
}

#[test]
fn async_scdc_runs_over_the_bench_bus() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let bus = bench.ddc();
    let mut scdc = SendScdcOverDdc::new(&bus).expect("the bench's bus serves SCDC");
    pollster::block_on(async {
        SendScdcTransport::write(&mut scdc, 0x20, 0x03)
            .await
            .unwrap();
        assert_eq!(SendScdcTransport::read(&scdc, 0x01).await, Ok(1));
    });
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
}

#[test]
fn errors_and_records_are_the_sync_handles() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let bus = bench.ddc();
    let mut phy = bench.phy();
    pollster::block_on(async {
        let error = SendDdcBus::write(&bus, 0x37, &[0]).await.unwrap_err();
        assert_eq!(
            error.kind,
            maquette::DdcErrorKind::NoAcknowledge(maquette::NoAcknowledgeSource::Address)
        );
        let error = SendTmdsPhy::start_output(&mut phy).await.unwrap_err();
        assert_eq!(error.kind, maquette::PhyErrorKind::NotConfigured);
        SendPhyPower::power_off(&mut phy).await.unwrap();
        SendFrlPhy::send_ltp(&mut phy, Default::default())
            .await
            .unwrap_err();
        let _ = SendTmdsPhy::capabilities(&phy);
        let _ = SendDdcBus::capabilities(&bus);
    });
    assert_eq!(bench.operations(), 4);
    assert_eq!(bench.transcript().count(), 4);
}

#[test]
fn nothing_happens_until_the_future_is_polled() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let bus = bench.ddc();
    let future = SendDdcBus::write(&bus, SCDC, &[0x20, 0x03]);
    assert_eq!(bench.sink().tmds_config(), Some(0));
    drop(future);
    assert_eq!(bench.operations(), 0);
}

#[test]
fn every_twin_method_reaches_the_bench() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let bus = bench.ddc();
    let mut phy = bench.phy();
    pollster::block_on(async {
        SendDdcBus::write(&bus, SCDC, &[0x20]).await.unwrap();
        let mut config = [0xEE];
        SendDdcBus::read(&bus, SCDC, &mut config).await.unwrap();
        assert_eq!(config, [0]);
        SendTmdsPhy::configure(&mut phy, high()).await.unwrap();
        SendTmdsPhy::start_output(&mut phy).await.unwrap();
        SendTmdsPhy::stop_output(&mut phy).await.unwrap();
        SendFrlPhy::set_frl_rate(&mut phy, HdmiForumFrl::Rate6Gbps4Lanes)
            .await
            .unwrap();
        SendFrlPhy::set_frl_output(&mut phy, maquette::FrlOutput::Active)
            .await
            .unwrap();
        SendFrlPhy::adjust_equalization(&mut phy, maquette::EqParams::new())
            .await
            .unwrap();
    });
    assert_eq!(bench.operations(), 8);
}

#[test]
fn a_simulated_clock_alone_is_an_async_delay() {
    let mut clock = maquette::SimClock::new();
    pollster::block_on(SendDelay::delay(&mut clock, Duration::from_millis(5)));
    assert_eq!(Clock::now(&clock).as_micros(), 5_000);
    let shared = &clock;
    pollster::block_on(SendDelay::delay(&mut &*shared, Duration::from_millis(5)));
    assert_eq!(Clock::now(&clock).as_micros(), 10_000);
    // The clock check's async form, without a bench.
    let check = maquette::check::ClockCheck::DEFAULT;
    pollster::block_on(check.run_async(&clock, &mut &clock)).assert_ok();
}
