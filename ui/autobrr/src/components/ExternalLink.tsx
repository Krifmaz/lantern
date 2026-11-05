/*
 * Copyright (c) 2021 - 2025, Ludvig Lundgren and the autobrr contributors.
 * SPDX-License-Identifier: GPL-2.0-or-later
 */

type ExternalLinkProps = {
  href: string;
  className?: string;
  children?: React.ReactNode;
};

export const ExternalLink = ({ href, className, children }: ExternalLinkProps) => (
  <a
    rel="noopener noreferrer"
    target="_blank"
    href={href}
    className={className}
  >
    {children}
  </a>
);

// LANTERN: no links to autobrr's own site (its docs); other sites' links stay
const isAutobrrSite = (href: string) => /^https:\/\/(www\.)?autobrr\.com(\/|$)/.test(href);

export const DocsLink = ({ href }: { href: string; }) => (isAutobrrSite(href) ? null :
  <ExternalLink href={href} className="text-blue-700 dark:text-blue-400 visited:text-blue-700 dark:visited:text-blue-400">{href}</ExternalLink>
);
