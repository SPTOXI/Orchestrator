// Sidebar panels whose features arrive in later phases.

interface Props {
  title: string;
  phase: string;
  description: string;
  items: string[];
}

export function PhasePlaceholder({ title, phase, description, items }: Props) {
  return (
    <div className="panel">
      <div className="panel-header">
        <span>{title}</span>
        <span className="badge">{phase}</span>
      </div>
      <div className="placeholder">
        <p>{description}</p>
        <ul>
          {items.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      </div>
    </div>
  );
}
