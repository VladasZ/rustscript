//! The same ratatui table as `ratatui_table`, drawn from a `#[tokio::main]`
//! script so the async path renders through the same code.

use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Cell;
use ratatui::widgets::Padding;
use ratatui::widgets::Row;
use ratatui::widgets::Sparkline;
use ratatui::widgets::Table;
use ratatui::widgets::Widget;

#[tokio::main]
async fn main() {
    let label = Style::new().fg(Color::Gray);
    let value = Style::new()
        .fg(Color::Rgb(20, 200, 90))
        .add_modifier(Modifier::BOLD);

    let rows = vec![
        Row::new(vec![
            Cell::from(Span::styled("host", label)),
            Cell::from(Span::styled("example.org", value)),
        ]),
        Row::new(vec![
            Cell::from(Span::styled("port", label)),
            Cell::from(Span::styled("443", value)),
        ]),
        Row::new(vec![Cell::from(""), Cell::from("")]),
    ];

    let title = Line::from(vec![Span::styled(" demo ", value)]);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Blue))
        .padding(Padding::new(1, 1, 0, 0))
        .title(title);
    let table = Table::new(rows, [Constraint::Length(6), Constraint::Min(1)])
        .column_spacing(2)
        .block(block);

    let area = Rect::new(0, 0, 26, 5);
    let mut buf = Buffer::empty(area);
    Widget::render(table, area, &mut buf);

    let spark = Sparkline::default()
        .data(vec![1, 3, 5, 8, 5, 3, 1])
        .style(Style::new().fg(Color::Yellow));
    Widget::render(spark, Rect::new(10, 3, 7, 1), &mut buf);

    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        println!("{}", line.trim_end());
    }

    if let Some(cell) = buf.cell((10, 1)) {
        println!(
            "value fg {:?} bold {}",
            cell.fg,
            cell.modifier.contains(Modifier::BOLD)
        );
    }
}
