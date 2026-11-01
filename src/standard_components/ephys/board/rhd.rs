//! Intan RHD2000 chips, as the boards drive them: their registers (bandwidth, DSP, power...), the command lists
//! the boards' FPGAs send them over and over (one sample per command), and telling which chip is on a data
//! stream. Ported from Intan's `Rhd2000Registers`, as used by the Open Ephys acquisition board plugins.

/// A chip's ID, in register 63.
pub const CHIP_RHD2132: i32 = 1;
pub const CHIP_RHD2216: i32 = 2;
pub const CHIP_RHD2164: i32 = 4;
/// Register 59 of an RHD2164 on its first MISO line (the second line, MISO B, reads 58).
pub const REGISTER_59_MISO_A: i32 = 53;
/// A 16-channel headstage on an RHD2132 uses its channels 8 to 23.
pub const RHD2132_16CH_OFFSET: usize = 8;

/// The register settings for a chip.
#[derive(Clone, Debug)]
pub struct Registers {
    sample_rate: f64,
    fast_settle: bool,
    mux_bias: i32,
    adc_buffer_bias: i32,
    temp_s1: i32,
    temp_s2: i32,
    temp_en: i32,
    dig_out: i32,
    dig_out_hiz: i32,
    dsp_en: i32,
    dsp_cutoff: i32,
    aux_en: i32,
    rh1_dac1: i32,
    rh1_dac2: i32,
    rh2_dac1: i32,
    rh2_dac2: i32,
    rl_dac1: i32,
    rl_dac2: i32,
    rl_dac3: i32,
}

impl Registers {
    pub fn new(sample_rate: f64) -> Self {
        let mut registers = Registers {
            sample_rate: 0.0,
            fast_settle: false,
            mux_bias: 0,
            adc_buffer_bias: 0,
            temp_s1: 0,
            temp_s2: 0,
            temp_en: 0,
            dig_out: 0,
            dig_out_hiz: 1,
            dsp_en: 1,
            dsp_cutoff: 0,
            aux_en: 1,
            rh1_dac1: 0,
            rh1_dac2: 0,
            rh2_dac1: 0,
            rh2_dac2: 0,
            rl_dac1: 0,
            rl_dac2: 0,
            rl_dac3: 0,
        };
        registers.set_sample_rate(sample_rate);
        registers.set_dsp_cutoff(1.0);
        registers.set_upper_bandwidth(10000.0);
        registers.set_lower_bandwidth(1.0);
        registers
    }

    /// The biases suiting the sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        (self.mux_bias, self.adc_buffer_bias) = match sample_rate {
            r if r < 3334.0 => (40, 32),
            r if r < 4001.0 => (40, 16),
            r if r < 5001.0 => (40, 8),
            r if r < 6251.0 => (32, 8),
            r if r < 8001.0 => (26, 8),
            r if r < 10001.0 => (18, 4),
            r if r < 12501.0 => (16, 3),
            r if r < 15001.0 => (7, 3),
            _ => (4, 2),
        };
    }

    pub fn set_fast_settle(&mut self, on: bool) {
        self.fast_settle = on;
    }

    /// Takes the auxiliary digital output out of its high-impedance state, low.
    pub fn set_dig_out_low(&mut self) {
        (self.dig_out, self.dig_out_hiz) = (0, 0);
    }

    pub fn enable_aux(&mut self, on: bool) {
        self.aux_en = on as i32;
    }

    pub fn enable_dsp(&mut self, on: bool) {
        self.dsp_en = on as i32;
    }

    /// The DSP high-pass filter's cut-off nearest `hz` (on a log scale). Returns the cut-off set.
    pub fn set_dsp_cutoff(&mut self, hz: f64) -> f64 {
        let cutoff = |n: i32| {
            let x = 2f64.powi(n);
            self.sample_rate * (x / (x - 1.0)).ln() / (2.0 * std::f64::consts::PI)
        };
        self.dsp_cutoff = if hz > cutoff(1) {
            1
        } else if hz < cutoff(15) {
            15
        } else {
            let distance = |n: i32| (hz.log10() - cutoff(n).log10()).abs();
            (1..16).min_by(|&a, &b| distance(a).total_cmp(&distance(b))).unwrap()
        };
        cutoff(self.dsp_cutoff)
    }

    /// The amplifiers' upper bandwidth nearest `hz` the on-chip resistors allow. Returns the bandwidth set.
    pub fn set_upper_bandwidth(&mut self, hz: f64) -> f64 {
        let hz = hz.min(30000.0);
        let log = hz.log10();
        let rh1_target = 0.9730 * 10f64.powf(8.0968 - 1.1892 * log + 0.04767 * log * log);
        let rh2_target = 1.0191 * 10f64.powf(8.1009 - 1.0821 * log + 0.03383 * log * log);
        let (rh1, d1, d2) = dacs(rh1_target, 2200.0, 600.0, 29400.0, 63, 31);
        (self.rh1_dac1, self.rh1_dac2) = (d1, d2);
        let (rh2, d1, d2) = dacs(rh2_target, 8700.0, 763.0, 38400.0, 63, 31);
        (self.rh2_dac1, self.rh2_dac2) = (d1, d2);
        let from = |r: f64, a: f64, b: f64, c0: f64, k: f64| {
            let c = c0 - (r / k).log10();
            10f64.powf((-b - (b * b - 4.0 * a * c).sqrt()) / (2.0 * a))
        };
        (from(rh1, 0.04767, -1.1892, 8.0968, 0.9730) * from(rh2, 0.03383, -1.0821, 8.1009, 1.0191)).sqrt()
    }

    /// The amplifiers' lower bandwidth nearest `hz` the on-chip resistors allow. Returns the bandwidth set.
    pub fn set_lower_bandwidth(&mut self, hz: f64) -> f64 {
        let hz = hz.min(1500.0);
        let log = hz.log10();
        let target = if hz < 4.0 {
            1.0061 * 10f64.powf(4.9391 - 1.2088 * log + 0.5698 * log * log + 0.1442 * log * log * log)
        } else {
            1.0061 * 10f64.powf(4.7351 - 0.5916 * log + 0.08482 * log * log)
        };
        let (base, dac3) = if hz < 0.15 { (3500.0 + 3_000_000.0, 1) } else { (3500.0, 0) };
        let (rl, d1, d2) = dacs(target, base, 175.0, 12700.0, 127, 63);
        (self.rl_dac1, self.rl_dac2, self.rl_dac3) = (d1, d2, dac3);
        lower_bandwidth_from_rl(rl)
    }

    fn register(&self, register: i32) -> i32 {
        let (adc_reference_bw, amp_vref_enable, adc_comparator_bias, adc_comparator_select) = (3, 1, 3, 2);
        let (vdd_sense_enable, mux_load, weak_miso, twos_comp, abs_mode) = (1, 0, 1, 0, 0);
        let (zcheck_dac_power, zcheck_load, zcheck_scale, zcheck_conn_all, zcheck_sel_pol, zcheck_en) =
            (1, 0, 0, 0, 0, 0);
        // Every amplifier powered.
        let powered = 0xff;
        match register {
            0 => {
                (adc_reference_bw << 6)
                    + ((self.fast_settle as i32) << 5)
                    + (amp_vref_enable << 4)
                    + (adc_comparator_bias << 2)
                    + adc_comparator_select
            }
            1 => (vdd_sense_enable << 6) + self.adc_buffer_bias,
            2 => self.mux_bias,
            3 => {
                (mux_load << 5)
                    + (self.temp_s2 << 4)
                    + (self.temp_s1 << 3)
                    + (self.temp_en << 2)
                    + (self.dig_out_hiz << 1)
                    + self.dig_out
            }
            4 => (weak_miso << 7) + (twos_comp << 6) + (abs_mode << 5) + (self.dsp_en << 4) + self.dsp_cutoff,
            5 => {
                (zcheck_dac_power << 6)
                    + (zcheck_load << 5)
                    + (zcheck_scale << 3)
                    + (zcheck_conn_all << 2)
                    + (zcheck_sel_pol << 1)
                    + zcheck_en
            }
            6 => 128,
            7 => 0,
            8 => self.rh1_dac1,
            9 => (self.aux_en << 7) + self.rh1_dac2,
            10 => self.rh2_dac1,
            11 => (self.aux_en << 7) + self.rh2_dac2,
            12 => self.rl_dac1,
            13 => (self.aux_en << 7) + (self.rl_dac3 << 6) + self.rl_dac2,
            14..=21 => powered,
            _ => -1,
        }
    }

    /// Writes every register and reads them back (with the chip's name and ID), then (if `calibrate`) starts
    /// the chip's ADC calibration. 60 commands.
    pub fn register_config(&self, calibrate: bool) -> Vec<u32> {
        let mut list = vec![read(63), read(63)];
        list.extend([0, 1, 2, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17].map(|r| write(r, self.register(r))));
        list.extend([63, 62, 61, 60, 59, 48, 49, 50, 51, 52, 53, 54, 55, 40, 41, 42, 43, 44].map(read));
        list.extend((0..18).map(read));
        list.push(if calibrate { 0x5500 } else { read(63) });
        list.extend([18, 19, 20, 21].map(|r| write(r, self.register(r))));
        list.push(read(63));
        list
    }

    /// Samples the auxiliary inputs on three of every four samples, and the temperature and supply sensors on
    /// the fourth (mostly). 60 commands.
    pub fn temperature_sensor(&mut self) -> Vec<u32> {
        let aux = [convert(32), convert(33), convert(34)];
        let mut list = Vec::new();
        self.temp_en = 1;
        list.extend(aux);
        let switch = |list: &mut Vec<u32>, registers: &mut Registers, s1, s2| {
            (registers.temp_s1, registers.temp_s2) = (s1, s2);
            list.push(write(3, registers.register(3)));
            list.extend(aux);
        };
        switch(&mut list, self, 1, 0);
        switch(&mut list, self, 1, 1);
        list.push(convert(49)); // temperature
        list.extend(aux);
        switch(&mut list, self, 0, 1);
        list.push(convert(49));
        list.extend(aux);
        switch(&mut list, self, 0, 0);
        list.push(convert(48)); // supply voltage
        for _ in 0..8 {
            list.extend(aux);
            list.push(read(63));
        }
        list
    }

    /// Writes register 3 (the auxiliary digital output) over and over, cycling the temperature sensor's
    /// switches as `temperature_sensor` does. 60 commands.
    pub fn update_dig_out(&mut self) -> Vec<u32> {
        let mut list = Vec::new();
        self.temp_en = 1;
        for (count, s1, s2) in [
            (3, None, None),
            (4, Some(1), Some(0)),
            (8, Some(1), Some(1)),
            (8, Some(0), Some(1)),
            (5, Some(0), Some(0)),
        ] {
            if let (Some(s1), Some(s2)) = (s1, s2) {
                (self.temp_s1, self.temp_s2) = (s1, s2);
            }
            list.extend((0..count).map(|_| write(3, self.register(3))));
        }
        list.extend((0..32).map(|_| write(3, self.register(3))));
        list
    }
}

/// The DAC steps (coarse, then fine) giving a resistance nearest `target`, as Intan's code chooses them:
/// returns the resistance and the fine and coarse step counts.
fn dacs(target: f64, base: f64, fine: f64, coarse: f64, fine_steps: i32, coarse_steps: i32) -> (f64, i32, i32) {
    let (mut actual, mut fine_count, mut coarse_count) = (base, 0, 0);
    for _ in 0..coarse_steps {
        if actual < target - (coarse - fine / 2.0) {
            actual += coarse;
            coarse_count += 1;
        }
    }
    for _ in 0..fine_steps {
        if actual < target - fine / 2.0 {
            actual += fine;
            fine_count += 1;
        }
    }
    (actual, fine_count, coarse_count)
}

fn lower_bandwidth_from_rl(rl: f64) -> f64 {
    let rl = rl.max(5100.0);
    let (a, b, c) = if rl < 30000.0 {
        (0.08482, -0.5916, 4.7351 - (rl / 1.0061).log10())
    } else {
        (0.3303, -1.2100, 4.9873 - (rl / 1.0061).log10())
    };
    10f64.powf((-b - (b * b - 4.0 * a * c).sqrt()) / (2.0 * a))
}

fn read(register: i32) -> u32 {
    0xc000 + ((register as u32) << 8)
}

fn write(register: i32, value: i32) -> u32 {
    0x8000 + ((register as u32) << 8) + value as u32
}

fn convert(channel: i32) -> u32 {
    (channel as u32) << 8
}

/// The chip on a data stream, from the results of `Registers::register_config` (the third command slot's
/// results, sample by sample, starting at the list's start): its ID and register 59, or `None` if no Intan chip
/// answered. Each command's result comes a sample later.
pub fn chip_id(results: &[u16]) -> Option<(i32, i32)> {
    let byte = |t: usize| results.get(t).map(|&r| r as u8);
    let name = [24, 25, 26, 32, 33, 34, 35, 36].map(byte);
    (name == [b'R', b'H', b'D', b'I', b'N', b'T', b'A', b'N'].map(Some))
        .then(|| (results[19] as i32, results[23] as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lists_are_as_long_as_intans() {
        let mut registers = Registers::new(30000.0);
        assert_eq!(registers.register_config(true).len(), 60);
        assert_eq!(registers.register_config(false).len(), 60);
        assert_eq!(registers.temperature_sensor().len(), 60);
        assert_eq!(registers.update_dig_out().len(), 60);
    }

    #[test]
    fn commands_are_encoded_as_intans() {
        let registers = Registers::new(30000.0);
        let list = registers.register_config(true);
        assert_eq!(list[0], 0xff00, "read register 63");
        assert_eq!(list[2] & 0xff00, 0x8000, "write register 0");
        assert_eq!(list[54], 0x5500, "calibrate");
    }

    #[test]
    fn bandwidths_come_out_near_those_asked() {
        let mut registers = Registers::new(30000.0);
        let upper = registers.set_upper_bandwidth(7500.0);
        assert!((upper - 7500.0).abs() / 7500.0 < 0.1, "{upper}");
        let lower = registers.set_lower_bandwidth(1.0);
        assert!((lower - 1.0).abs() < 0.2, "{lower}");
        let dsp = registers.set_dsp_cutoff(0.5);
        assert!((0.3..0.8).contains(&dsp), "{dsp}");
    }

    #[test]
    fn chips_are_told_by_their_answers() {
        let mut results = vec![0u16; 60];
        for (t, c) in [(24, b'R'), (25, b'H'), (26, b'D'), (32, b'I'), (33, b'N'), (34, b'T'), (35, b'A'), (36, b'N')] {
            results[t] = c as u16;
        }
        results[19] = CHIP_RHD2164 as u16;
        results[23] = REGISTER_59_MISO_A as u16;
        assert_eq!(chip_id(&results), Some((CHIP_RHD2164, REGISTER_59_MISO_A)));
        assert_eq!(chip_id(&[0; 60]), None);
    }
}
