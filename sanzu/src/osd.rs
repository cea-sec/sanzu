use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Dimensions, Point, Size};
use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::primitives::{rectangle::Rectangle, PrimitiveStyle};
use embedded_graphics::Pixel;

use embedded_graphics::{
    mono_font::{ascii::FONT_7X13, MonoTextStyle},
    prelude::*,
    text::Text,
};

pub struct TestDisplay<'a> {
    pub width: u32,
    pub height: u32,
    pub buffer: &'a mut [u8],
}

impl<'a> TestDisplay<'a> {
    pub fn new(width: u32, height: u32, buffer: &'a mut Vec<u8>) -> TestDisplay<'a> {
        TestDisplay {
            width,
            height,
            buffer,
        }
    }
}

impl<'a> Dimensions for TestDisplay<'a> {
    fn bounding_box(&self) -> Rectangle {
        Rectangle {
            top_left: Point { x: 0, y: 0 },
            size: Size {
                width: self.width,
                height: self.height,
            },
        }
    }
}

impl<'a> DrawTarget for TestDisplay<'a> {
    type Color = Rgb888;
    type Error = ();
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for pixel in pixels {
            let point = pixel.0;
            let color = pixel.1;
            let x = if point.x >= 0 {
                point.x as u32
            } else {
                continue;
            };
            let y = if point.y >= 0 {
                point.y as u32
            } else {
                continue;
            };

            if x < self.width && y < self.height {
                let index = ((y * self.width + x) * 4) as usize;
                self.buffer[index] = color.r();
                self.buffer[index + 1] = color.g();
                self.buffer[index + 2] = color.b();
            }
        }
        Ok(())
    }

    fn fill_contiguous<I>(&mut self, _area: &Rectangle, _colors: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let point = area.top_left;
        let size = area.size;
        // drop whole rectangle is neg position (event if rectangle is partly on the screen)
        if point.x < 0 {
            return Ok(());
        }
        if point.y < 0 {
            return Ok(());
        }

        if point.x as u32 >= self.width {
            return Ok(());
        }
        if point.y as u32 >= self.height {
            return Ok(());
        }

        let x = point.x as u32;
        let y = point.y as u32;

        let remaining_width = self.width - x;
        let remaining_height = self.height - y;

        let max_width = if size.width > remaining_width {
            remaining_width
        } else {
            size.width
        };

        let max_height = if size.height > remaining_height {
            remaining_height
        } else {
            size.height
        };

        for j in y..y + max_height {
            let mut index = (((j * self.width) + x) * 4) as usize;
            for _ in 0..max_width {
                self.buffer[index] = color.r();
                self.buffer[index + 1] = color.g();
                self.buffer[index + 2] = color.b();
                index += 4;
            }
        }
        Ok(())
    }
    fn clear(&mut self, _color: Self::Color) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub fn draw_text(display: &mut TestDisplay, text: &str, x: i32, y: i32) {
    Rectangle::new(Point::new(x, y - 9), Size::new(7 * (text.len() as u32), 13))
        .into_styled(PrimitiveStyle::with_fill(Rgb888::BLACK))
        .draw(display)
        .expect("Cannot draw rectangle");

    let style = MonoTextStyle::new(&FONT_7X13, Rgb888::WHITE);
    let text_front = Text::new(text, Point::new(x, y), style);
    text_front.draw(display).expect("Cannot draw text");
}
