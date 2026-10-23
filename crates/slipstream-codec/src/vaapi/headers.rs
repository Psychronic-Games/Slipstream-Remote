//! H.264 parameter sets and slice headers, written to match exactly the
//! parameters `VaEncoder` gives the driver. Drivers such as Mesa's radeonsi
//! leave these to the application ("packed headers") and encode only the
//! slice data. Each header is an Annex-B NAL unit with its start code;
//! without emulation prevention, which libva's packed headers insert, unless
//! [`escape`] is applied for a stream assembled here.

/// Exp-Golomb and fixed-width fields, most significant bit first.
#[derive(Default)]
pub(super) struct Bits {
    bytes: Vec<u8>,
    bits: usize,
}

impl Bits {
    pub fn u(&mut self, width: u32, value: u32) {
        for i in (0..width).rev() {
            if self.bits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> i) & 1 == 1 {
                *self.bytes.last_mut().expect("a byte") |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
    }

    pub fn flag(&mut self, on: bool) {
        self.u(1, on as u32);
    }

    /// Unsigned Exp-Golomb.
    pub fn ue(&mut self, value: u32) {
        let v = value as u64 + 1;
        let len = 64 - v.leading_zeros();
        self.u(len - 1, 0);
        for i in (0..len).rev() {
            self.u(1, ((v >> i) & 1) as u32);
        }
    }

    /// Signed Exp-Golomb.
    pub fn se(&mut self, value: i32) {
        self.ue(if value > 0 { (value as u32) * 2 - 1 } else { value.unsigned_abs() * 2 });
    }

    /// rbsp_trailing_bits: a one, then zeros to the byte boundary.
    pub fn trailing(&mut self) {
        self.u(1, 1);
        while !self.bits.is_multiple_of(8) {
            self.u(1, 0);
        }
    }

    pub fn len_bits(&self) -> u32 {
        self.bits as u32
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// What the headers describe.
pub(super) struct Stream {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// A start code and the NAL header byte.
fn nal(ref_idc: u32, kind: u32) -> Bits {
    let mut b = Bits::default();
    b.u(32, 1);
    b.u(1, 0);
    b.u(2, ref_idc);
    b.u(5, kind);
    b
}

pub(super) const LOG2_MAX_FRAME_NUM: u32 = 16;

/// Constrained Baseline, level 5.1, picture order type 2, one reference.
pub(super) fn sps(s: &Stream) -> Bits {
    let (mbw, mbh) = (s.width.div_ceil(16), s.height.div_ceil(16));
    let mut b = nal(3, 7);
    b.u(8, 66); // profile_idc: Baseline
    b.u(8, 0b1100_0000); // constraint_set0 and set1: Constrained Baseline
    b.u(8, 51); // level_idc
    b.ue(0); // seq_parameter_set_id
    b.ue(LOG2_MAX_FRAME_NUM - 4);
    b.ue(2); // pic_order_cnt_type
    b.ue(1); // max_num_ref_frames
    b.flag(false); // gaps_in_frame_num_value_allowed_flag
    b.ue(mbw - 1);
    b.ue(mbh - 1);
    b.flag(true); // frame_mbs_only_flag
    b.flag(true); // direct_8x8_inference_flag
    let (crop_right, crop_bottom) = ((mbw * 16 - s.width) / 2, (mbh * 16 - s.height) / 2);
    b.flag(crop_right > 0 || crop_bottom > 0);
    if crop_right > 0 || crop_bottom > 0 {
        b.ue(0);
        b.ue(crop_right);
        b.ue(0);
        b.ue(crop_bottom);
    }
    b.flag(true); // vui_parameters_present_flag
    b.flag(false); // aspect_ratio_info_present_flag
    b.flag(false); // overscan_info_present_flag
    b.flag(false); // video_signal_type_present_flag
    b.flag(false); // chroma_loc_info_present_flag
    b.flag(true); // timing_info_present_flag
    b.u(32, 1); // num_units_in_tick
    b.u(32, s.fps.max(1) * 2); // time_scale
    b.flag(false); // fixed_frame_rate_flag: screens change when they change
    b.flag(false); // nal_hrd_parameters_present_flag
    b.flag(false); // vcl_hrd_parameters_present_flag
    b.flag(false); // pic_struct_present_flag
    b.flag(true); // bitstream_restriction_flag
    b.flag(true); // motion_vectors_over_pic_boundaries_flag
    b.ue(0); // max_bytes_per_pic_denom
    b.ue(0); // max_bits_per_mb_denom
    b.ue(15); // log2_max_mv_length_horizontal
    b.ue(15); // log2_max_mv_length_vertical
    b.ue(0); // max_num_reorder_frames: show each frame as it arrives
    b.ue(1); // max_dec_frame_buffering
    b.trailing();
    b
}

/// CAVLC, one reference, QP 26, deblocking controlled per slice.
pub(super) fn pps() -> Bits {
    let mut b = nal(3, 8);
    b.ue(0); // pic_parameter_set_id
    b.ue(0); // seq_parameter_set_id
    b.flag(false); // entropy_coding_mode_flag: CAVLC
    b.flag(false); // bottom_field_pic_order_in_frame_present_flag
    b.ue(0); // num_slice_groups_minus1
    b.ue(0); // num_ref_idx_l0_default_active_minus1
    b.ue(0); // num_ref_idx_l1_default_active_minus1
    b.flag(false); // weighted_pred_flag
    b.u(2, 0); // weighted_bipred_idc
    b.se(0); // pic_init_qp_minus26
    b.se(0); // pic_init_qs_minus26
    b.se(0); // chroma_qp_index_offset
    b.flag(true); // deblocking_filter_control_present_flag
    b.flag(false); // constrained_intra_pred_flag
    b.flag(false); // redundant_pic_cnt_present_flag
    b.trailing();
    b
}

/// The header of a whole-picture slice; the driver appends the slice data.
pub(super) fn slice(idr: bool, frame_num: u32, idr_pic_id: u16) -> Bits {
    let mut b = nal(if idr { 3 } else { 2 }, if idr { 5 } else { 1 });
    b.ue(0); // first_mb_in_slice
    b.ue(if idr { 7 } else { 5 }); // slice_type: all I / all P in this picture
    b.ue(0); // pic_parameter_set_id
    b.u(LOG2_MAX_FRAME_NUM, frame_num);
    if idr {
        b.ue(idr_pic_id as u32);
    } else {
        b.flag(false); // num_ref_idx_active_override_flag
        b.flag(false); // ref_pic_list_modification_flag_l0
    }
    if idr {
        b.flag(false); // no_output_of_prior_pics_flag
        b.flag(false); // long_term_reference_flag
    } else {
        b.flag(false); // adaptive_ref_pic_marking_mode_flag
    }
    b.se(0); // slice_qp_delta
    b.ue(0); // disable_deblocking_filter_idc
    b.se(0); // slice_alpha_c0_offset_div2
    b.se(0); // slice_beta_offset_div2
    b
}

/// Inserts emulation prevention bytes after each start code's NAL header,
/// for parameter sets added to the stream here rather than by the driver.
pub(super) fn escape(nal: &[u8]) -> Vec<u8> {
    let (head, body) = nal.split_at(5.min(nal.len()));
    let mut out = head.to_vec();
    let mut zeros = 0;
    for &byte in body {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_codes() {
        let mut b = Bits::default();
        for v in [0, 1, 2, 3, 7] {
            b.ue(v);
        }
        // 1 010 011 00100 0001000
        assert_eq!(b.len_bits(), 1 + 3 + 3 + 5 + 7);
        assert_eq!(b.into_bytes(), [0b1010_0110, 0b0100_0001, 0b0000_0000]);
        let mut b = Bits::default();
        for v in [0, 1, -1, 2] {
            b.se(v);
        }
        // 1 010 011 00100
        assert_eq!(b.into_bytes(), [0b1010_0110, 0b0100_0000]);
    }

    #[test]
    fn emulation_prevention_after_the_header() {
        assert_eq!(escape(&[0, 0, 0, 1, 0x67, 0, 0, 1, 0, 0, 0]), [0, 0, 0, 1, 0x67, 0, 0, 3, 1, 0, 0, 3, 0]);
    }
}
