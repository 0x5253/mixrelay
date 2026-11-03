use super::BridgeError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedbackQueue {
    Input,
    Output,
}

#[derive(Clone, Copy, Debug)]
pub struct ClockFeedback {
    pub queue: FeedbackQueue,
    pub epoch: u64,
    /// Current and desired FIFO occupancy in the selected queue's rate domain.
    pub queued_frames: usize,
    pub target_frames: usize,
    /// Actual advanced frames since last observation, in the same rate domain.
    /// Never use worker iteration count, wall-clock sleep time or signal level.
    pub elapsed_frames: usize,
    /// False while priming, stopped, disconnected or clock position is unknown.
    /// This freezes the integrator; reset explicitly on a discontinuity.
    pub clock_running: bool,
}

pub(super) struct DriftController {
    integral: f64,
    correction: f64,
    limit: f64,
}

impl DriftController {
    pub(super) fn new(limit: f64) -> Self {
        Self {
            integral: 0.0,
            correction: 0.0,
            limit,
        }
    }
    pub(super) fn reset(&mut self) {
        self.integral = 0.0;
        self.correction = 0.0;
    }
    pub(super) fn correction(&self) -> f64 {
        self.correction
    }
    /// Adopts an externally measured correction; occupancy feedback continues from it.
    pub(super) fn set(&mut self, ppm: f64) -> f64 {
        self.correction = ppm.clamp(-self.limit, self.limit);
        self.integral = self.correction;
        self.correction
    }
    pub(super) fn observe(
        &mut self,
        feedback: ClockFeedback,
        rate: u32,
    ) -> Result<f64, BridgeError> {
        if feedback.target_frames == 0
            || feedback.queued_frames > 16_777_216
            || feedback.target_frames > 16_777_216
            || feedback.elapsed_frames > rate as usize
            || (feedback.clock_running && feedback.elapsed_frames == 0)
        {
            return Err(BridgeError::InvalidFeedback);
        }
        if !feedback.clock_running {
            return Ok(self.correction);
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "validated frame counts below 2^24"
        )]
        let error_seconds =
            (feedback.queued_frames as f64 - feedback.target_frames as f64) / f64::from(rate);
        #[expect(
            clippy::cast_precision_loss,
            reason = "elapsed frames bounded to one second"
        )]
        let seconds = feedback.elapsed_frames as f64 / f64::from(rate);
        // PI in physical units, independent of callback size. A full FIFO needs
        // fewer output frames per input frame. Integral clamping + conditional
        // integration prevents windup during saturation; slew avoids abrupt pitch.
        let proportional = -0.20 * error_seconds * 1e6;
        let candidate =
            (self.integral - 0.01 * error_seconds * seconds * 1e6).clamp(-self.limit, self.limit);
        let raw = proportional + candidate;
        if raw.abs() <= self.limit || raw.signum() != (-error_seconds).signum() {
            self.integral = candidate;
        }
        let target = (proportional + self.integral).clamp(-self.limit, self.limit);
        self.correction += (target - self.correction).clamp(-200.0 * seconds, 200.0 * seconds);
        Ok(self.correction)
    }
}
