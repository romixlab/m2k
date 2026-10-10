"""Drive an Analog Devices ADALM2000 (M2K) over the network: scope, signal generator, power supplies,
voltmeter, logic analyzer, calibration. Pure Rust underneath (no libiio, no libm2k).
"""

class M2kError(Exception):
    """Anything the M2K or the connection refused: a bad setting, an unreachable device, a device error."""

class Analog:
    """Summary of an analog capture of one channel."""

    samples: int
    min: float
    max: float
    mean: float
    rms: float
    """RMS including the DC part."""
    ac_rms: float
    """RMS of what is left without the mean."""
    vpp: float
    frequency: float | None
    """From rising crossings of the middle between min and max; `None` without at least two of them."""
    periods: int
    duty: float | None
    """Share of the time above the middle, over whole periods."""
    clipped: int
    """Samples at the end of the ADC range: the signal is larger than the input range."""

class ScopeCapture:
    """A capture of both scope channels, in volts (`M2k.scope`, `M2k.voltmeter`)."""

    rate: float
    """Samples per second actually used (the nearest the ADC's divider can hit to what was asked for)."""
    ch1: list[float]
    ch2: list[float]
    ch1_stats: Analog
    ch2_stats: Analog
    range1: float
    range2: float
    calibrated: bool
    """The scope was calibrated since power-up."""

class Digital:
    """Summary of one digital pin over a `LogicCapture`."""

    channel: int
    state: str
    """`low`, `high` or `toggling`."""
    rising: int
    falling: int
    high: float
    """Share of the samples that are high."""
    frequency: float | None
    min_pulse: float | None
    """Shortest time between two edges, in seconds."""

class LogicCapture:
    """A capture of the 16 digital pins (`M2k.logic`), summarized for the pins asked for."""

    rate: float
    words: list[int]
    """One integer per sample, bit N = DIO N."""
    pins: list[Digital]

class PsuStatus:
    """Measured voltage and enabled state of both user supply rails (`M2k.psu_status`/`psu_set`/`psu_off`)."""

    pos_enabled: bool
    pos_measured: float
    neg_enabled: bool
    neg_measured: float

class DmmReading:
    """One board monitor reading (`M2k.dmm`): a supply rail, temperature or FPGA voltage."""

    channel: str
    value: float
    unit: str

class Corrections:
    """The corrections the M2K holds (`M2k.cal_run`/`cal_status`/`cal_reset`)."""

    calibrated: bool
    """Something differs from the power-on values: a calibration ran since power-up."""
    adc_offset: list[int | None]
    """Scope offset per channel ([ch1, ch2]); `None` on firmware before 0.26."""
    adc_gain: list[float]
    dac_offset: list[int]
    dac_gain: list[float]

class M2k:
    """A connection to an ADALM2000, over its iiod daemon (`ip:192.168.2.1:30431` by default)."""

    def __init__(self, uri: str | None = None, timeout: float = 5.0) -> None:
        """Connects to the M2K at `uri` (`ip:192.168.2.1` when omitted) and powers up the scope front end, as
        libm2k does when it opens an M2K. Raises `M2kError` if it is unreachable or not an ADALM2000."""

    def temperature(self) -> float:
        """Board temperature next to the converters, in °C."""

    def scope(
        self, samples: int, rate: float, range1: float = 25.0, range2: float | None = None
    ) -> ScopeCapture:
        """Captures `samples` samples per channel at (close to) `rate` S/s, at `range1`/`range2` volts (25 or
        2.5; `range2` defaults to `range1`)."""

    def voltmeter(self, range: float = 25.0) -> ScopeCapture:
        """DC and AC rms on both channels, at `range` volts (25 or 2.5): 100 ms at 100 kS/s."""

    def logic(self, samples: int, rate: float, pins: list[int] | None = None) -> LogicCapture:
        """Captures `samples` samples of the 16 digital pins at (close to) `rate` S/s, summarized for `pins`
        (all 16 when omitted). Free-running; no trigger."""

    def gen_start(self, channels: list[int], rate: float, volts: list[float]) -> None:
        """Loads `volts` as a repeating buffer into `channels` (0 = W1, 1 = W2) and starts them; the output
        lasts until `gen_stop` or the connection closes. Build the samples with numpy, e.g. a sine:
        `amp * np.sin(2 * np.pi * np.arange(n) / n)`."""

    def gen_stop(self, channel: int) -> None:
        """Switches an output off: amplifier powered down, buffer freed."""

    def psu_status(self) -> PsuStatus:
        """Measured voltage and enabled state of both rails."""

    def psu_set(self, pos: float | None = None, neg: float | None = None) -> PsuStatus:
        """Sets and switches on the given rails (`None` leaves a rail as it is): `pos` 0 to 5 V, `neg` -5 to
        0 V."""

    def psu_off(self, pos: bool = False, neg: bool = False) -> PsuStatus:
        """Switches rails off (both when neither `pos` nor `neg` is set)."""

    def dmm(self, filter: list[str] | None = None) -> list[DmmReading]:
        """Every monitor channel (supply rails, temperature, FPGA voltages; not the scope inputs) whose label
        contains one of `filter` (all of them when omitted)."""

    def cal_run(self, adc_only: bool = False) -> Corrections:
        """Self-calibrates (about 5 s): scope inputs, then the outputs too unless `adc_only` (W1/W2 are
        driven to ~2.5 V for a moment)."""

    def cal_status(self) -> Corrections:
        """The corrections the M2K currently holds."""

    def cal_reset(self) -> Corrections:
        """Back to the power-on values: no correction at all."""
