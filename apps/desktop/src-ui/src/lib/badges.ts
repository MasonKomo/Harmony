export const MAX_BADGES_PER_USER = 5

export interface BadgeDefinition {
  code: string
  label: string
  src: string
}

export const BADGE_MANIFEST: BadgeDefinition[] = [
  {
    code: 'rainbow-core',
    label: 'Rainbow Core',
    src: '/badges/rainbow-badge.png',
  },
  {
    code: 'party-parrot',
    label: 'Party Parrot',
    src: '/badges/partyparrot.png',
  },
]

const BADGE_BY_CODE = new Map(BADGE_MANIFEST.map((badge) => [badge.code, badge]))

const sanitizeBadgeCode = (value: string) => value.trim().toLowerCase()

export const isBadgeCodeValid = (code: string) => BADGE_BY_CODE.has(sanitizeBadgeCode(code))

export const resolveBadgeByCode = (code: string): BadgeDefinition | undefined =>
  BADGE_BY_CODE.get(sanitizeBadgeCode(code))

export const normalizeBadgeCodes = (codes: string[]): string[] => {
  const normalized: string[] = []
  const seen = new Set<string>()

  for (const rawCode of codes) {
    const code = sanitizeBadgeCode(rawCode)
    if (!code || seen.has(code) || !BADGE_BY_CODE.has(code)) {
      continue
    }
    normalized.push(code)
    seen.add(code)
    if (normalized.length >= MAX_BADGES_PER_USER) {
      break
    }
  }

  return normalized
}
