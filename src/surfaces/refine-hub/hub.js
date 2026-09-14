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
