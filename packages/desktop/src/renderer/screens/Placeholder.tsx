export function Placeholder({ title }: { title: string }) {
  return (
    <>
      <header className="page-header">
        <div>
          <h1 className="page-title">{title}</h1>
          <p className="page-sub">Coming soon.</p>
        </div>
      </header>
    </>
  );
}
