/*
 * Copyright (c) 2021 - 2025, Ludvig Lundgren and the autobrr contributors.
 * SPDX-License-Identifier: GPL-2.0-or-later
 */

// import { Link, NavLink } from "react-router-dom";

import { Link } from '@tanstack/react-router'
import { useTranslation } from "react-i18next";

import { classNames } from "@utils";

import { NAV_ROUTES } from "./_shared";

// LANTERN: the instance name in place of autobrr's logo (src/lantern/Wordmark.tsx)
import Logo from "@app/lantern/Wordmark";

export const LeftNav = () => {
  const { t } = useTranslation("common");

  return (
    <div className="flex items-center">
      <div className="shrink-0 flex items-center">
        <Link to="/">
          <Logo className="h-10" />
        </Link>
      </div>
      <div className="sm:ml-3 hidden sm:block">
        <div className="flex items-baseline space-x-4">
          {NAV_ROUTES.map((item, itemIdx) => (
            <Link
              key={item.labelKey + itemIdx}
              to={item.path}
              params={{}}
            >
              {({ isActive }) => {
                return (
                  <>
                    <span className={
                      classNames(
                        "hover:bg-gray-200 dark:hover:bg-gray-800 hover:text-gray-900 dark:hover:text-white px-3 py-2 rounded-2xl text-sm font-medium",
                        "transition-colors duration-200",
                        isActive
                          ? "text-black dark:text-gray-50 font-bold"
                          : "text-gray-600 dark:text-gray-500"
                      )
                    }>{t(item.labelKey)}</span>
                  </>
                )
              }}
            </Link>
          ))}
          {/* LANTERN: no link to autobrr's docs */}
        </div>
      </div>
    </div>
  );
};
