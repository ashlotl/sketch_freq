#[derive(Clone, Debug)]
pub struct AABB {
    top_left: (usize, usize),
    bottom_right: (usize, usize),
}

impl AABB {
    /// Creates new AABB if input coordinates are valid; otherwise returns None.
    /// Input coordinates are valid when `bottom_right` has components greater than `top_left`, and nonequal to `top_left`.
    pub fn new(top_left: (usize, usize), bottom_right: (usize, usize)) -> Option<Self> {
        let dx = bottom_right.0 as isize - top_left.0 as isize;
        let dy = bottom_right.1 as isize - top_left.1 as isize;
        if dx <= 0 || dy <= 0 {
            return None;
        }
        Some(Self {
            top_left,
            bottom_right,
        })
    }

    pub fn left(&self) -> usize {
        self.top_left.0
    }

    pub fn top(&self) -> usize {
        self.top_left.1
    }

    pub fn right(&self) -> usize {
        self.bottom_right.0
    }

    pub fn bottom(&self) -> usize {
        self.bottom_right.1
    }

    pub fn top_left(&self) -> (usize, usize) {
        self.top_left.clone()
    }

    pub fn bottom_right(&self) -> (usize, usize) {
        self.bottom_right.clone()
    }

    pub fn expand_left(&mut self, amount: isize) {
        self.top_left.0 =
            ((self.top_left.0 as isize - amount).max(0) as usize).min(self.bottom_right.0 - 1);
    }

    pub fn expand_right(&mut self, amount: isize) {
        self.bottom_right.0 =
            ((self.bottom_right.0 as isize + amount).max(0) as usize).max(self.top_left.0 + 1)
    }

    /// Computes area.
    pub fn area(&self) -> usize {
        (self.bottom_right.0 - self.top_left.0) * (self.bottom_right.1 - self.top_left.1)
    }

    /// Computes height.
    pub fn height(&self) -> usize {
        self.bottom_right.1 - self.top_left.1
    }

    /// Computes width.
    pub fn width(&self) -> usize {
        self.bottom_right.0 - self.top_left.0
    }
}
