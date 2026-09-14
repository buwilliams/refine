const search = document.querySelector('#hub-search');
const sections = [...document.querySelectorAll('nav details')];
const initial = sections.map(section => section.open);
search.addEventListener('input', () => {
  const query = search.value.trim().toLowerCase();
  let count = 0;
  sections.forEach((section, index) => {
    let matches = 0;
    section.querySelectorAll('a').forEach(link => {
      link.hidden = !link.textContent.toLowerCase().includes(query);
      if (!link.hidden) matches++;
    });
    count += matches;
    section.hidden = !matches;
    section.open = query ? !!matches : initial[index];
  });
  document.querySelector('#hub-search-status').textContent = query ? `${count} matching pages` : '';
});
// Markdown headings use GitHub-style anchors, including links in migrated pages.
const used = new Map();
document.querySelectorAll('h1,h2,h3,h4,h5,h6').forEach(heading => {
  if (heading.id) return;
  const slug = heading.textContent.toLowerCase().replace(/[^\p{L}\p{N}\s_-]/gu, '').trim().replace(/\s/g, '-');
  const count = used.get(slug) || 0;
  used.set(slug, count + 1);
  heading.id = slug + (count ? `-${count}` : '');
});
if (location.hash) document.getElementById(decodeURIComponent(location.hash.slice(1)))?.scrollIntoView();

// Keep long documents easy to scan without adding more navigation on the home page.
const outline = document.querySelector('.page-outline');
const headings = [...document.querySelectorAll('article h2')];
if (outline && headings.length > 1 && !document.body.classList.contains('home-page')) {
  const links = headings.map(heading => {
    const link = document.createElement('a');
    link.textContent = heading.textContent;
    link.href = `${location.pathname}#${encodeURIComponent(heading.id)}`;
    outline.querySelector('[data-outline]').append(link);
    return link;
  });
  outline.hidden = false;
  const observer = new IntersectionObserver(entries => {
    const entry = entries.find(entry => entry.isIntersecting);
    if (!entry) return;
    links.forEach((link, index) => {
      if (headings[index] === entry.target) link.setAttribute('aria-current', 'location');
      else link.removeAttribute('aria-current');
    });
  }, {rootMargin: '-80px 0px -60% 0px'});
  headings.forEach(heading => observer.observe(heading));
}
const menu = document.querySelector('.menu-toggle');
if (menu) {
  document.body.classList.add('js');
  menu.hidden = false;
  menu.addEventListener('click', () => {
    const open = menu.getAttribute('aria-expanded') !== 'true';
    menu.setAttribute('aria-expanded', String(open));
    document.querySelector('#hub-navigation').classList.toggle('is-open', open);
    if (open) search.focus();
  });
}
