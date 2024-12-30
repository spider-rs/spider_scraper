# spider_scraper

CSS Scraper tuned for spider.

## Usage 

1. `cargo add spider_scraper`

```rs
use scraper::{Html, Selector};

let html = r#"
    <ul>
        <li>Foo</li>
        <li>Bar</li>
        <li>Baz</li>
    </ul>
"#;

let fragment = Html::parse_fragment(html);
let selector = Selector::parse("li").unwrap();

for element in fragment.select(&selector) {
    assert_eq!("li", element.value().name());
}
```