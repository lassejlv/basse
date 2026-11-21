export function SectionHead({ index, label, title, id }: { index: string; label: string; title: string; id: string }) {
  return (
    <div className="flex flex-col gap-3 md:gap-4">
      <p className="text-xs text-mist md:text-[13px]">
        {index} · {label}
      </p>
      <h2 id={id} className="text-2xl leading-[31px] font-medium tracking-[-0.02em] md:text-[32px] md:leading-10">
        {title}
      </h2>
    </div>
  )
}
