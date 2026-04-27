export function PosterCardSkeleton() {
  return (
    <div className="w-full">
      <div className="aspect-poster rounded-lg skeleton" />
      <div className="mt-2 h-3 w-3/4 skeleton" />
      <div className="mt-1 h-2.5 w-1/3 skeleton" />
    </div>
  );
}
