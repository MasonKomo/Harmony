import { cn } from '@/lib/utils'
import { resolveBadgeByCode } from '@/lib/badges'

interface BadgeIconsProps {
  badgeCodes: string[]
  size?: 'sm' | 'md'
  className?: string
}

const SIZE_CLASSES: Record<NonNullable<BadgeIconsProps['size']>, string> = {
  sm: 'size-4',
  md: 'size-5',
}

export function BadgeIcons({ badgeCodes, size = 'sm', className }: BadgeIconsProps) {
  const badges = badgeCodes
    .map((code) => resolveBadgeByCode(code))
    .filter((badge): badge is NonNullable<typeof badge> => Boolean(badge))

  if (badges.length === 0) {
    return null
  }

  return (
    <span className={cn('inline-flex items-center gap-1', className)}>
      {badges.map((badge) => (
        <img
          key={badge.code}
          src={badge.src}
          alt={badge.label}
          title={badge.label}
          loading="lazy"
          className={cn(
            'rounded-sm border border-border/50 bg-background/90 object-cover shadow-[0_0_0_1px_rgba(0,0,0,0.08)]',
            SIZE_CLASSES[size]
          )}
        />
      ))}
    </span>
  )
}
